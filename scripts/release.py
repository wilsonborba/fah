#!/usr/bin/env python3
"""Audit locked dependency notices and assemble a verified emulated preview candidate."""
import argparse
import gzip
import hashlib
import importlib.util
import json
import os
import pathlib
import re
import shutil
import subprocess
import tarfile
import tomllib

ROOT = pathlib.Path(__file__).resolve().parents[1]
_release_spec = importlib.util.spec_from_file_location('fah_release_context', ROOT / 'scripts/release_context.py')
RELEASE_CONTEXT = importlib.util.module_from_spec(_release_spec)
_release_spec.loader.exec_module(RELEASE_CONTEXT)
TARGET = 'x86_64-unknown-linux-gnu'
ALLOWED = {'MIT', 'Apache-2.0', 'BSD-3-Clause', 'Unlicense', 'Unicode-3.0'}
# The candidate package is generated build output, next to the executable but outside Cargo's profile directories.
OUTPUT = ROOT / 'target' / 'fah-release'


def load_preparation():
    spec = importlib.util.spec_from_file_location('fah_prepare_dependencies', ROOT / 'scripts/prepare-dependencies.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


PREPARATION = load_preparation()


def verified_sources():
    """Compare the ignored dependency trees with the pinned sources and tracked patches (#60).

    This is the preparation script's own check. It fetches and changes nothing; unknown edits stop the release."""
    return PREPARATION.verify(ROOT)


def command(*args):
    return subprocess.check_output(args, cwd=ROOT, text=True).strip()


def digest(path):
    return hashlib.sha256(pathlib.Path(path).read_bytes()).hexdigest()


def normal_build_graph(metadata):
    nodes = {node['id']: node for node in metadata['resolve']['nodes']}
    packages = {package['id']: package for package in metadata['packages']}
    seen = set()
    pending = [metadata['resolve']['root']]
    while pending:
        identity = pending.pop()
        if identity in seen:
            continue
        seen.add(identity)
        for dependency in nodes[identity]['deps']:
            if any(kind['kind'] != 'dev' for kind in dependency['dep_kinds']):
                pending.append(dependency['pkg'])
    return sorted((packages[identity] for identity in seen if identity != metadata['resolve']['root']), key=lambda package: (package['name'], package['version']))


def validate_expression(expression):
    if not expression:
        raise ValueError('dependency has no declared license expression')
    tokens = set(re.findall(r'[A-Za-z0-9][A-Za-z0-9.-]*', expression.replace('/', ' OR ')))
    unknown = tokens - ALLOWED - {'OR', 'AND'}
    if unknown:
        raise ValueError(f'unreviewed license expression: {expression}; unknown={sorted(unknown)}')


def license_files(package, policy):
    validate_expression(package['license'])
    source = pathlib.Path(package['manifest_path']).parent
    files = [path for path in sorted(source.iterdir()) if path.is_file() and path.name.lower().startswith(('license', 'copying', 'notice'))]
    if package.get('license_file'):
        declared = pathlib.Path(package['license_file'])
        if not declared.is_absolute():
            declared = source / declared
        if declared not in files:
            files.append(declared)
    result = []
    if files:
        for path in files:
            if not path.read_bytes().strip():
                raise ValueError(f'empty license text: {package["name"]}/{path.name}')
            result.append(dict(name=path.name, sha256=digest(path), origin='published-package-text', source=f'{package.get("repository") or package["source"]} version {package["version"]}: {path.name}', _path=path))
    else:
        key = package['name'] + '@' + package['version']
        reviewed = policy.get(key)
        if not reviewed or reviewed['license'] != package['license']:
            raise ValueError(f'missing unreviewed license text: {key}')
        for entry in reviewed['files']:
            path = (ROOT / entry['path']).resolve()
            if not path.is_relative_to(ROOT) or digest(path) != entry['sha256']:
                raise ValueError(f'license fallback checksum/path mismatch: {key}')
            result.append(dict(name=path.name, sha256=entry['sha256'], origin=entry['origin'], source=entry['source'], note=reviewed['note'], _path=path))
    if 'Unicode-3.0' in package['license'] and not any('unicode' in entry['name'].lower() for entry in result):
        raise ValueError('Unicode-3.0 notice is missing')
    return result


def inventory():
    metadata = json.loads(command('cargo', 'metadata', '--locked', '--format-version', '1', '--features', 'hosted-direct', '--filter-platform', TARGET))
    policy = json.loads((ROOT / 'assets/licenses/fallbacks.json').read_text())
    records = []
    for package in normal_build_graph(metadata):
        records.append(dict(name=package['name'], version=package['version'], license=package['license'], repository=package.get('repository'), source=package.get('source') or 'maintained local source; see project pins/patches', authors=package['authors'], texts=license_files(package, policy)))
    return metadata, records


def public_record(record):
    return {**record, 'texts': [{key: value for key, value in text.items() if not key.startswith('_')} for text in record['texts']]}


def notices(records):
    lines = ['# Third-party notices', '', 'Generated from the locked Linux x86_64 hosted normal/build dependency graph.',
             'Build-time dependencies are included conservatively; this is a source/notice inventory,',
             'not a claim that every listed package is linked into the executable.', '',
             'Declared license expressions and published text contents are preserved.',
             'Reviewed fallback filenames identify their source package/provenance.',
             'Full texts are bundled under `licenses/<name>-<version>/` in the candidate.',
             'Reviewed fallbacks for incomplete archives live in `assets/licenses/fallbacks.json`;',
             'the trait-ffi supplied standard text is explicitly identified as such.', '',
             'FAH modifies CrabUSB and dma-api; see `NOTICE` and the maintained patches.',
             'The additional Unicode-3.0 text is retained for unicode-ident.', '',
             '| Package | Declared license | Texts and origins |', '| --- | --- | --- |']
    for record in records:
        texts = ', '.join(f'`{text["name"]}` ({text["origin"]})' for text in record['texts'])
        repository = record['repository'] or f'https://crates.io/crates/{record["name"]}/{record["version"]}'
        lines.append(f'| [{record["name"]} {record["version"]}]({repository}) | {record["license"]} | {texts} |')
    return '\n'.join(lines) + '\n'


TRACE_PHASES={'passive','chosen_reset','slot','bsr','descriptor_at_default','assigned_address','compatibility_plan','negative_capabilities','noop','equivalent_explicit','equivalent_convenience'}
FAULT_CASES={'timeout_logical_cancel_late_completion','endpoint_abort_restart','uncertain_stop','close_pending','eof_pending','sigint_wait_then_shutdown','sigterm_pending','endpoint_reset_state','endpoint_reset_preserve','guest_process_death','disconnect_pending'}


def verified_runs(paths, commit, binary_hash):
    runs = []
    for path in paths:
        path = path.expanduser().resolve()
        if not (path / '.fah-lab-run').is_file():
            raise ValueError('unrecognized evidence run')
        result = json.loads((path / 'result.json').read_text())
        manifest = json.loads((path / 'manifest.json').read_text())
        if not result['success'] or result['qemu_exit'] != 0 or manifest['commit'] != commit:
            raise ValueError(f'evidence is failed or from another release commit: {path.name}')
        if manifest['sources'].get(str(ROOT / 'target/release/fah')) != binary_hash:
            raise ValueError('the clean pinned rebuild differs from the VM-tested executable; build the payload with README\'s clean pinned command, prepare a new image and repeat the required suites')
        if result['suite'] == 'explicit':
            validation = json.loads((path / 'trace-validation.json').read_text())
            if len(validation) != 11 or {row.get('phase')for row in validation}!=TRACE_PHASES or not all(row['verified'] for row in validation):
                raise ValueError('independent trace windows incomplete')
        if result['suite'] == 'faults':
            matrix = json.loads((path / 'failure-matrix.json').read_text())
            if len(matrix) != 11 or {row.get('name')for row in matrix}!=FAULT_CASES or not all(row['verified'] for row in matrix):
                raise ValueError('guest failure matrix incomplete')
        if result['suite']=='m8':
            validation=json.loads((path/'m8-validation.json').read_text())
            cases={row.get('name')for row in validation['cases']if row.get('verified')}
            if not validation['verified'] or not validation['independent_trace'] or not RELEASE_CONTEXT.M8_CASES.issubset(cases):
                raise ValueError('M8 guest evidence incomplete')
        runs.append(dict(id=path.name, suite=result['suite'], commit=commit, binary_sha256=binary_hash, _path=path))
    suites = [run['suite'] for run in runs]
    if tuple(map(int,RELEASE_CONTEXT.package_version(ROOT).split('.')[:2]))>=(0,2) and 'm8'not in suites:
        raise ValueError('M8 release requires fresh M8 guest evidence')
    if len({run['id'] for run in runs}) != len(runs) or suites.count('smoke') < 2 or not {'feasibility', 'fixture', 'explicit', 'faults'}.issubset(suites):
        raise ValueError('candidate requires distinct fresh feasibility/fixture/smoke/smoke/explicit/faults runs')
    return runs


def clean_release():
    return RELEASE_CONTEXT.require_clean_release(ROOT, command)


def normalize_tar(info):
    info.uid = info.gid = info.mtime = 0
    info.uname = info.gname = ''
    return info


def pinned_toolchain():
    """Return rust-toolchain.toml's channel. A rustup override can change the executable bytes (#63)."""
    channel = tomllib.loads((ROOT / 'rust-toolchain.toml').read_text())['toolchain']['channel']
    override = os.environ.get('RUSTUP_TOOLCHAIN')
    if override not in (None, channel):
        raise ValueError(f'unset RUSTUP_TOOLCHAIN={override}; the candidate builds only with the pinned {channel} toolchain')
    return channel


def build(sources):
    """Rebuild the executable from nothing with the pinned toolchain; refuse it if the trees changed.

    Cargo reuses artifacts by file time and treats same-version toolchains as equal (#63)."""
    pinned_toolchain()
    subprocess.run(['cargo', 'clean', '--locked', '--release'], cwd=ROOT, check=True)
    subprocess.run(['cargo', 'build', '--locked', '--release', '--features', 'hosted-direct'], cwd=ROOT, check=True)
    if verified_sources() != sources:
        raise ValueError('dependency patches or trees changed during the build')


def package(output, evidence, metadata, records, sources):
    version = clean_release()
    if not command('rustc', '--version').startswith(f'rustc {pinned_toolchain()} '):
        raise ValueError('unexpected release toolchain')
    if pathlib.Path(metadata['target_directory']).resolve() != ROOT / 'target':
        raise ValueError('candidate uses the default target directory shared with the VM recipe')
    build(sources)
    binary = ROOT / 'target/release/fah'
    binary_hash = digest(binary)
    if command(str(binary), '--version') != 'fah ' + version:
        raise ValueError('binary version mismatch')
    command(str(binary), '--help')
    capabilities = json.loads(command(str(binary), 'capabilities'))
    if capabilities['schema_version'] != 1 or capabilities['physical_validated']:
        raise ValueError('unexpected capability contract')
    commit = command('git', 'rev-parse', 'HEAD')
    runs = verified_runs(evidence, commit, binary_hash)
    output = output.expanduser().absolute()
    if output == pathlib.Path('/') or output.is_relative_to('/tmp') or output.is_symlink():
        raise ValueError('select a dedicated disk-backed candidate directory')
    output.mkdir(parents=True, exist_ok=True)
    if command('findmnt', '-no', 'FSTYPE', '--target', str(output)) in ('tmpfs', 'ramfs', 'devtmpfs'):
        raise ValueError('candidate artifacts belong on disk')
    stage = output / ('fah-v' + version + '-' + commit[:12])
    if stage.exists():
        if stage.is_symlink() or not (stage / '.fah-release-generated').is_file():
            raise ValueError('refusing to replace unmarked candidate directory')
        shutil.rmtree(stage)
    stage.mkdir()
    (stage / '.fah-release-generated').touch()
    (stage / 'bin').mkdir()
    shutil.copy2(binary, stage / 'bin/fah')
    for name in ['README.md', 'CHANGELOG.md', 'LICENSE-MIT', 'LICENSE-APACHE', 'NOTICE', 'THIRD_PARTY_NOTICES.md', 'Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml']:
        shutil.copy2(ROOT / name, stage / name)
    shutil.copytree(ROOT / 'patches', stage / 'patches')
    shutil.copytree(ROOT / 'assets/licenses', stage / 'reviewed-license-fallbacks')
    for record in records:
        destination = stage / 'licenses' / (record['name'] + '-' + record['version'])
        destination.mkdir(parents=True)
        for text in record['texts']:
            shutil.copy2(text['_path'], destination / text['name'])
    (stage / 'dependency-licenses.json').write_text(json.dumps([public_record(record) for record in records], indent=2, ensure_ascii=False) + '\n')
    (stage / 'capabilities.json').write_text(json.dumps(capabilities, indent=2) + '\n')
    evidence_files = ['manifest.json', 'command.json', 'result.json', 'serial.log', 'events.json', 'xhci.trace', 'pci.json', 'qemu.stderr', 'trace-validation.json', 'failure-matrix.json']
    for run in runs:
        destination = stage / 'evidence' / run['id']
        destination.mkdir(parents=True)
        for name in evidence_files:
            if (run['_path'] / name).is_file():
                shutil.copy2(run['_path'] / name, destination / name)
    source = subprocess.check_output(['git', 'archive', '--format=tar', '--prefix=fah-source/', 'HEAD'], cwd=ROOT)
    (stage / 'source.tar.gz').write_bytes(gzip.compress(source, mtime=0))
    provenance = dict(version=version, target=TARGET, source_commit=commit, source_tree=command('git', 'rev-parse', 'HEAD^{tree}'), binary_sha256=binary_hash, cargo_lock_sha256=digest(ROOT / 'Cargo.lock'), patch_sha256={path.name: digest(path) for path in sorted((ROOT / 'patches').glob('*.patch'))}, dependency_sources=dict(sources, verified='before the dependency inventory and after the build'), rustc=command('rustc', '-Vv'), cargo=command('cargo', '--version'), rustflags=os.environ.get('RUSTFLAGS', ''), dynamic_libraries=re.sub(r'0x[0-9a-fA-F]+','<runtime-address>',command('ldd', str(binary))), build='cargo clean --locked --release && cargo build --locked --release --features hosted-direct', normal_build_dependencies=len(records), evidence=[{key: value for key, value in run.items() if key != '_path'} for run in runs], physical_validated=False)
    (stage / 'provenance.json').write_text(json.dumps(provenance, indent=2, ensure_ascii=False) + '\n')
    notes = f'''# FAH v{version}: emulated preview candidate

Source commit: {commit}
Binary SHA256: {binary_hash}
Target: {TARGET}; tested Rust1.91.1 / Debian13 userspace.

The candidate includes the executable, source snapshot, maintained patches,
license texts/notices, provenance/checksums and six fresh guest runs from this exact
commit/binary. It validates control/bulk/interrupt I/O, eleven independent trace
windows and eleven fault/recovery cases. See README and evidence manifests.

Before the dependency inventory and again after a clean build with the pinned
toolchain, the packaging tool compared every file of the prepared CrabUSB and
dma-api trees with the pinned revision/archive plus the maintained patches
(provenance.json, dependency_sources). That clean build reproduced the VM-tested
executable byte for byte.

Physical Intel behavior, physical process-death/isolation/timing, full DMA-model
soundness, electrical Pico equivalence and iPad/A5 compatibility remain unverified.
M7/#32-34 remain blocked/pending. Warm reset, W1C/interface variants and batching
retain the narrower evidence levels recorded in capabilities.json.

Rebuild from the pinned repository/source using README's locked commands. Host
userspace/toolchain/library differences can change binary bytes; provenance records
the tested inputs rather than promising identical binaries on arbitrary hosts.

Main/tag promotion follows the owner's conditional authorization after VM checks.
Publication of a hosted release is a separate action; this script does not publish,
create tags, merge branches or send an upstream PR.
'''
    (stage / 'RELEASE-NOTES.md').write_text(notes)
    sums = '\n'.join(f'{digest(path)}  {path.relative_to(stage)}' for path in sorted(stage.rglob('*')) if path.is_file() and path.name != 'SHA256SUMS') + '\n'
    (stage / 'SHA256SUMS').write_text(sums)
    archive = output / f'fah-v{version}-{TARGET}.tar.gz'
    with archive.open('wb') as raw, gzip.GzipFile(fileobj=raw, mode='wb', filename='', mtime=0) as compressed, tarfile.open(fileobj=compressed, mode='w') as tar:
        for path in sorted(stage.rglob('*')):
            tar.add(path, arcname=str(path.relative_to(stage)), recursive=False, filter=normalize_tar)
    (output / (archive.name + '.sha256')).write_text(digest(archive) + '  ' + archive.name + '\n')
    print(json.dumps(dict(candidate=str(stage), archive=str(archive), archive_sha256=digest(archive), binary_sha256=binary_hash, source_commit=commit, evidence_runs=len(runs))))


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['audit', 'candidate'])
    parser.add_argument('--write-notices', action='store_true')
    parser.add_argument('--output', type=pathlib.Path, default=OUTPUT)
    parser.add_argument('--evidence', type=pathlib.Path, action='append', default=[])
    args = parser.parse_args(argv)
    # Cargo metadata, license texts and the build all read these trees: verify them first.
    sources = verified_sources()
    metadata, records = inventory()
    expected = notices(records)
    path = ROOT / 'THIRD_PARTY_NOTICES.md'
    if args.write_notices:
        path.write_text(expected)
    elif not path.is_file() or path.read_text() != expected:
        raise ValueError('dependency notices are missing/stale; run audit --write-notices')
    if args.action == 'candidate':
        package(args.output, args.evidence, metadata, records, sources)
    else:
        print(json.dumps(dict(reviewed_normal_build_dependencies=len(records), texts=sum(len(record['texts']) for record in records), notice_consistent=True, dependency_sources_verified=True)))


if __name__ == '__main__':
    try:
        main()
    except Exception as error:
        raise SystemExit('release error: ' + str(error))
