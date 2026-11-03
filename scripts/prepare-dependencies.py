#!/usr/bin/env python3
"""Prepare owned, pinned sources. Refuse unknown changes instead of overwriting them.

With --check, only verify the prepared trees: no network access and no changes."""
import argparse
import hashlib
import io
import json
import os
import pathlib
import shutil
import subprocess
import tarfile
import tempfile
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[1]
REV = 'e83b56b0672460b400105f3855222af58c03af87'
CRAB_URL = 'https://github.com/drivercraft/CrabUSB.git'
DMA_SHA = '6ce802d0c429c6966bea151e8e78eb5906d5b5e2cdebd0e960097cbe3654a3db'
DMA_URL = 'https://static.crates.io/crates/dma-api/dma-api-0.7.3.crate'
# Untracked top-level CrabUSB entries: Git metadata and the outputs of tests/backend.py.
# The FAH build reads none of them. Every other file must equal the pinned tree plus the patch.
CRAB_OUTPUTS = frozenset({'.git', 'target', 'Cargo.lock'})


class Sources:
    """Pinned inputs and generated dependency trees of one checkout."""

    def __init__(self, root):
        self.cache = root / '.fah-cache'
        self.crab = self.cache / 'CrabUSB'
        self.crab_patch = root / 'patches' / 'crabusb-hosted-backend.patch'
        self.crab_previous = root / 'patches' / 'crabusb-previous.sha256'
        self.dma = self.cache / 'dma-api'
        self.dma_archive = self.cache / 'dma-api-0.7.3.crate'
        self.dma_stamp = self.cache / 'dma-api-tree.json'
        self.dma_patch = root / 'patches' / 'dma-api-vfio.patch'


def digest(data):
    return hashlib.sha256(data).hexdigest()


def git(directory, *args):
    return subprocess.run(['git', '-C', str(directory), *args], check=True, text=True, stdout=subprocess.PIPE).stdout


def tree(directory, skip=frozenset()):
    """Hash every regular file below directory; skip names top-level entries. Links and special files are refused."""
    files = {}
    for current, folders, names in os.walk(directory):
        base = pathlib.Path(current)
        if base == directory:
            folders[:] = [name for name in folders if name not in skip]
            names = [name for name in names if name not in skip]
        for name in folders + names:
            path = base / name
            if path.is_symlink() or not (path.is_dir() or path.is_file()):
                raise SystemExit(f'Unexpected non-regular dependency entry: {path.relative_to(directory)}')
        for name in names:
            path = base / name
            files[str(path.relative_to(directory))] = digest(path.read_bytes())
    return files


def fingerprint(files):
    return digest(json.dumps(files, sort_keys=True).encode())


def describe(expected, actual):
    rows = [f'{path} (changed)' for path in sorted(expected.keys() & actual.keys()) if expected[path] != actual[path]]
    rows += [f'{path} (missing)' for path in sorted(expected.keys() - actual.keys())]
    rows += [f'{path} (unexpected)' for path in sorted(actual.keys() - expected.keys())]
    return ', '.join(rows[:5]) + (f' and {len(rows) - 5} more' if len(rows) > 5 else '')


def apply(patch, directory):
    """Apply a maintained patch to an exported tree. Git must not see an enclosing checkout (#61)."""
    environment = {key: value for key, value in os.environ.items() if key not in ('GIT_DIR', 'GIT_WORK_TREE')}
    environment['GIT_CEILING_DIRECTORIES'] = str(directory.parent)
    subprocess.run(['git', 'apply', str(patch)], cwd=directory, env=environment, check=True)


def crab_expected(sources, workdir):
    export = workdir / 'CrabUSB'
    export.mkdir()
    archive = subprocess.run(['git', '-C', str(sources.crab), 'archive', '--format=tar', REV], check=True, stdout=subprocess.PIPE).stdout
    with tarfile.open(fileobj=io.BytesIO(archive)) as package:
        package.extractall(export, filter='data')
    apply(sources.crab_patch, export)
    return tree(export)


def dma_expected(sources, workdir):
    payload = sources.dma_archive.read_bytes()
    if digest(payload) != DMA_SHA:
        raise SystemExit('Cached DMA archive checksum mismatch')
    with tarfile.open(fileobj=io.BytesIO(payload)) as package:
        package.extractall(workdir, filter='data')
    expected = workdir / 'dma-api-0.7.3'
    apply(sources.dma_patch, expected)
    return expected, tree(expected)


def verify(root=ROOT):
    """Prove that both prepared trees equal the pinned sources plus the maintained patches.

    The expected trees are rebuilt in a temporary directory and compared file by file.
    Nothing is fetched or changed; a difference stops the caller and stays in place."""
    sources = Sources(root)
    for patch in (sources.crab_patch, sources.dma_patch):
        if not patch.is_file():
            raise SystemExit(f'Missing dependency patch: {patch.name}')
    if not (sources.crab / '.git').exists() or not sources.dma.is_dir() or not sources.dma_archive.is_file():
        raise SystemExit('Pinned dependency sources are not prepared; run scripts/prepare-dependencies.py')
    if git(sources.crab, 'rev-parse', 'HEAD').strip() != REV:
        raise SystemExit('Unexpected source revision; refusing.')
    with tempfile.TemporaryDirectory() as temporary:
        workdir = pathlib.Path(temporary)
        crab = crab_expected(sources, workdir)
        dma = dma_expected(sources, workdir)[1]
    tracked = {path.split('/', 1)[0] for path in crab}
    for name, expected, actual in (('CrabUSB', crab, tree(sources.crab, CRAB_OUTPUTS - tracked)), ('dma-api', dma, tree(sources.dma))):
        if actual != expected:
            raise SystemExit(f'Unexpected changes in cached {name} source (kept in place): {describe(expected, actual)}')
    return {'CrabUSB': dict(revision=REV, patch_sha256=digest(sources.crab_patch.read_bytes()), files=len(crab), tree_sha256=fingerprint(crab)),
            'dma-api': dict(archive_sha256=DMA_SHA, patch_sha256=digest(sources.dma_patch.read_bytes()), files=len(dma), tree_sha256=fingerprint(dma))}


def prepare_crab(sources):
    if not sources.crab_patch.is_file():
        raise SystemExit('Missing dependency patch; no source changes were made.')
    if not sources.crab.exists():
        sources.cache.mkdir(exist_ok=True)
        subprocess.run(['git', 'clone', '--no-checkout', CRAB_URL, str(sources.crab)], check=True)
        git(sources.crab, 'checkout', '--detach', REV)
    if git(sources.crab, 'rev-parse', 'HEAD').strip() != REV:
        raise SystemExit('Unexpected source revision; refusing to patch.')
    reverse = subprocess.run(['git', '-C', str(sources.crab), 'apply', '--reverse', '--check', str(sources.crab_patch)], capture_output=True)
    if reverse.returncode:
        diff = git(sources.crab, 'diff', '--binary', 'HEAD')
        if git(sources.crab, 'ls-files', '--others', '--exclude-standard').strip():
            raise SystemExit('Unexpected untracked dependency files.')
        if diff:
            if digest(diff.encode()) not in sources.crab_previous.read_text().splitlines():
                raise SystemExit('Unexpected source changes; refusing to patch.')
            subprocess.run(['git', '-C', str(sources.crab), 'apply', '--reverse', '-'], input=diff, text=True, check=True)
        git(sources.crab, 'apply', '--check', str(sources.crab_patch))
        git(sources.crab, 'apply', str(sources.crab_patch))
    if git(sources.crab, 'diff', '--binary', 'HEAD') != sources.crab_patch.read_text():
        raise SystemExit('Unexpected modifications beyond the maintained patch.')
    if git(sources.crab, 'ls-files', '--others', '--exclude-standard').strip():
        raise SystemExit('Unexpected untracked dependency files.')


def prepare_dma(sources):
    """Unpack maintained dma-api source only from the checksum-pinned registry archive."""
    if not sources.dma_archive.exists():
        with urllib.request.urlopen(DMA_URL, timeout=60) as response:
            payload = response.read()
        if digest(payload) != DMA_SHA:
            raise SystemExit('DMA archive checksum mismatch')
        sources.dma_archive.write_bytes(payload)
    with tempfile.TemporaryDirectory() as temporary:
        expected, expected_tree = dma_expected(sources, pathlib.Path(temporary))
        if sources.dma.exists() and tree(sources.dma) != expected_tree:
            if not sources.dma_stamp.exists() or tree(sources.dma) != json.loads(sources.dma_stamp.read_text()):
                raise SystemExit('Unexpected changes in cached dma-api source')
            shutil.rmtree(sources.dma)
        if not sources.dma.exists():
            shutil.copytree(expected, sources.dma)
        sources.dma_stamp.write_text(json.dumps(expected_tree, sort_keys=True))


def prepare(root=ROOT):
    sources = Sources(root)
    prepare_crab(sources)
    prepare_dma(sources)
    return verify(root)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true', help='verify the prepared trees without network access or changes')
    args = parser.parse_args(argv)
    summary = verify() if args.check else prepare()
    state = 'verified' if args.check else 'ready'
    print(f'Pinned CrabUSB hosted backend is {state}:', summary['CrabUSB']['revision'])
    print(f'Pinned dma-api VFIO access patch is {state}:', summary['dma-api']['archive_sha256'])


if __name__ == '__main__':
    main()
