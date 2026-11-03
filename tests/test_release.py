"""Release policy regressions; these tests never start a VM, access hardware or use the network."""
import contextlib
import hashlib
import importlib.util
import io
import json
import os
import pathlib
import shutil
import subprocess
import tarfile
import tempfile
import unittest
from unittest import mock

ROOT=pathlib.Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location('fah_release',ROOT/'scripts/release.py')
release=importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)
prepare=release.PREPARATION
REAL_RUN,REAL_OUTPUT=subprocess.run,subprocess.check_output
PATCHED='pub fn maintained() {}\n'


def git(*args,cwd):
    return REAL_RUN(['git',*args],cwd=cwd,check=True,capture_output=True,text=True).stdout


def snapshot(root):
    """Exact files and links, Git objects and refs included. Only Git's stat cache may be refreshed."""
    state={}
    for path in sorted(root.rglob('*')):
        name=str(path.relative_to(root))
        if path.is_symlink():state[name]=('link',os.readlink(path))
        elif path.is_file() and not name.endswith('.git/index'):state[name]=path.read_bytes()
    return state


class Fixture:
    """A miniature checkout with its own pinned CrabUSB/dma-api inputs; never the real cache."""
    def __init__(self,directory):
        self.root=root=pathlib.Path(directory)/'checkout'
        self.crab=crab=root/'.fah-cache/CrabUSB'
        self.dma=root/'.fah-cache/dma-api'
        (crab/'usb-host/src').mkdir(parents=True)
        (root/'scripts').mkdir()
        (root/'patches').mkdir()
        git('init','-q',cwd=root)  # like a real clone, a Git checkout encloses the cache (#61)
        (crab/'.gitignore').write_text('target/\nCargo.lock\ndebug/\n')
        (crab/'usb-host/src/lib.rs').write_text('pub fn pinned() {}\n')
        (crab/'usb-host/src/other.rs').write_text('pub fn untouched() {}\n')
        git('init','-q',cwd=crab)
        git('add','.',cwd=crab)
        git('-c','user.name=fixture','-c','user.email=fixture@example.invalid','-c','commit.gpgsign=false','-c','core.hooksPath=/dev/null','commit','-q','-m','pin',cwd=crab)
        self.rev=git('rev-parse','HEAD',cwd=crab).strip()
        (crab/'usb-host/src/lib.rs').write_text(PATCHED)
        (root/'patches/crabusb-hosted-backend.patch').write_text(git('diff','--binary','HEAD',cwd=crab))
        git('checkout','-q','--','.',cwd=crab)
        (root/'patches/crabusb-previous.sha256').write_text('')
        upstream=pathlib.Path(directory)/'dma-api-0.7.3'
        (upstream/'src').mkdir(parents=True)
        (upstream/'src/lib.rs').write_text('pub fn upstream() {}\n')
        crate=io.BytesIO()
        with tarfile.open(fileobj=crate,mode='w:gz')as archive:archive.add(upstream,arcname='dma-api-0.7.3')
        (root/'.fah-cache/dma-api-0.7.3.crate').write_bytes(crate.getvalue())
        self.dma_sha=hashlib.sha256(crate.getvalue()).hexdigest()
        (root/'patches/dma-api-vfio.patch').write_text('diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-pub fn upstream() {}\n+'+PATCHED)


class SourceTrees(unittest.TestCase):
    def setUp(self):
        self.enterContext(mock.patch.object(prepare.urllib.request,'urlopen',side_effect=AssertionError('network access')))

    def prepared(self):
        fixture=Fixture(self.enterContext(tempfile.TemporaryDirectory()))
        self.enterContext(mock.patch.multiple(prepare,REV=fixture.rev,DMA_SHA=fixture.dma_sha))
        with contextlib.chdir(fixture.root):prepare.prepare(fixture.root)
        return fixture


class LicensePolicy(unittest.TestCase):
    def test_dev_only_dependencies_are_excluded(self):
        metadata=dict(resolve=dict(root='root',nodes=[dict(id='root',deps=[dict(pkg='runtime',dep_kinds=[dict(kind=None)]),dict(pkg='build',dep_kinds=[dict(kind='build')]),dict(pkg='dev',dep_kinds=[dict(kind='dev')])]),dict(id='runtime',deps=[]),dict(id='build',deps=[]),dict(id='dev',deps=[])]),packages=[dict(id=name,name=name,version='1')for name in ['root','runtime','build','dev']])
        self.assertEqual([package['name']for package in release.normal_build_graph(metadata)],['build','runtime'])

    def test_missing_unreviewed_text_and_unknown_expression_fail(self):
        with tempfile.TemporaryDirectory()as directory:
            package=dict(name='unknown',version='1',license='MIT',manifest_path=str(pathlib.Path(directory)/'Cargo.toml'),license_file=None)
            with self.assertRaisesRegex(ValueError,'missing unreviewed'):release.license_files(package,{})
        with self.assertRaisesRegex(ValueError,'unreviewed license'):release.validate_expression('Unknown-License')
        release.validate_expression('(MIT OR Apache-2.0) AND Unicode-3.0')

    def test_reviewed_fallbacks_are_checksum_pinned_and_truthfully_classified(self):
        policy=json.loads((ROOT/'assets/licenses/fallbacks.json').read_text())
        for record in policy.values():
            for entry in record['files']:
                self.assertEqual(hashlib.sha256((ROOT/entry['path']).read_bytes()).hexdigest(),entry['sha256'])
        self.assertTrue(any(entry['origin']=='supplied-standard-text-with-upstream-attribution'for entry in policy['trait-ffi@0.2.11']['files']))
        with tempfile.TemporaryDirectory()as directory:
            package=dict(name='dma-api',version='0.7.3',license='Apache-2.0',manifest_path=str(pathlib.Path(directory)/'Cargo.toml'),license_file=None)
            policy['dma-api@0.7.3']['files'][0]['sha256']='bad'
            with self.assertRaisesRegex(ValueError,'checksum'):release.license_files(package,policy)

    def test_unicode_notice_cannot_be_omitted(self):
        with tempfile.TemporaryDirectory()as directory:
            source=pathlib.Path(directory)
            (source/'LICENSE-MIT').write_text('MIT text fixture')
            package=dict(name='unicode-fixture',version='1',license='MIT AND Unicode-3.0',manifest_path=str(source/'Cargo.toml'),repository='https://example.invalid',source=None,license_file=None)
            with self.assertRaisesRegex(ValueError,'Unicode'):release.license_files(package,{})


class EvidenceGate(unittest.TestCase):
    def test_candidate_refuses_stale_binary_commit_or_incomplete_suite_set(self):
        with tempfile.TemporaryDirectory()as directory:
            paths=[]
            for index,suite in enumerate(['feasibility','fixture','smoke','smoke','explicit','faults']):
                path=pathlib.Path(directory)/str(index);path.mkdir();paths.append(path)
                (path/'.fah-lab-run').touch()
                (path/'result.json').write_text(json.dumps(dict(success=True,qemu_exit=0,suite=suite)))
                (path/'manifest.json').write_text(json.dumps(dict(commit='rev',sources={str(ROOT/'target/release/fah'):'bin'})))
                if suite=='explicit':(path/'trace-validation.json').write_text(json.dumps([dict(phase=name,verified=True)for name in sorted(release.TRACE_PHASES)]))
                if suite=='faults':(path/'failure-matrix.json').write_text(json.dumps([dict(name=name,verified=True)for name in sorted(release.FAULT_CASES)]))
            self.assertEqual(len(release.verified_runs(paths,'rev','bin')),6)
            for candidates,commit,binary in [(paths,'old','bin'),(paths,'rev','changed'),(paths[:-1],'rev','bin')]:
                with self.assertRaises(ValueError):release.verified_runs(candidates,commit,binary)
            with self.assertRaises(ValueError):release.verified_runs(paths+[paths[0]],'rev','bin')


class PreparedSourceGate(SourceTrees):
    def test_check_accepts_exact_trees_and_changes_nothing(self):
        fixture=self.prepared()
        (fixture.crab/'target/debug').mkdir(parents=True)
        (fixture.crab/'target/debug/output').write_text('private backend test output')
        (fixture.crab/'Cargo.lock').write_text('private backend test lock')
        before=snapshot(fixture.root)
        summary=prepare.verify(fixture.root)
        self.assertEqual(snapshot(fixture.root),before)
        self.assertEqual((summary['CrabUSB']['revision'],summary['dma-api']['archive_sha256']),(fixture.rev,fixture.dma_sha))
        self.assertEqual([(tree/'src/lib.rs').read_text()for tree in (fixture.crab/'usb-host',fixture.dma)],[PATCHED,PATCHED])

    def test_unknown_changes_are_refused_and_kept_by_check_and_prepare(self):
        def hide(fixture):
            git('update-index','--skip-worktree','usb-host/src/other.rs',cwd=fixture.crab)
            (fixture.crab/'usb-host/src/other.rs').write_text('pub fn hidden() {}\n')
        def ignored(fixture):
            (fixture.crab/'usb-host/src/debug').mkdir()
            (fixture.crab/'usb-host/src/debug/mod.rs').write_text('pub fn ignored() {}\n')
        mutations=dict(
            patched_file=lambda fixture:(fixture.crab/'usb-host/src/lib.rs').write_text('pub fn local() {}\n'),
            untracked_file=lambda fixture:(fixture.crab/'usb-host/src/new.rs').write_text('pub fn new() {}\n'),
            ignored_source=ignored,
            hidden_by_index_flag=hide,
            deleted_file=lambda fixture:(fixture.crab/'usb-host/src/other.rs').unlink(),
            dma_file=lambda fixture:(fixture.dma/'src/lib.rs').write_text('pub fn local() {}\n'),
            dma_extra_file=lambda fixture:(fixture.dma/'src/extra.rs').write_text('pub fn extra() {}\n'),
            dma_link=lambda fixture:(fixture.dma/'link.rs').symlink_to('src/lib.rs'))
        for name,mutate in mutations.items():
            with self.subTest(name):
                fixture=self.prepared()
                mutate(fixture)
                before=snapshot(fixture.root)
                for operation in (prepare.verify,prepare.prepare):
                    with contextlib.chdir(fixture.root),self.assertRaisesRegex(SystemExit,'Unexpected'):operation(fixture.root)
                    self.assertEqual(snapshot(fixture.root),before)

    def test_check_never_fetches_or_creates_missing_sources(self):
        fixture=self.prepared()
        shutil.rmtree(fixture.dma)
        (fixture.root/'.fah-cache/dma-api-0.7.3.crate').unlink()
        before=snapshot(fixture.root)
        with self.assertRaisesRegex(SystemExit,'not prepared'):prepare.verify(fixture.root)
        self.assertEqual(snapshot(fixture.root),before)

    def test_preparation_does_not_depend_on_the_working_directory(self):
        fixture=self.prepared()
        with contextlib.chdir(fixture.root/'scripts'):
            prepare.prepare(fixture.root)  # over the stamped tree: #61 replaced it with unpatched source
            self.assertEqual((fixture.dma/'src/lib.rs').read_text(),PATCHED)
            shutil.rmtree(fixture.dma)
            prepare.prepare(fixture.root)  # from nothing: #61 copied the unpatched archive
        self.assertEqual((fixture.dma/'src/lib.rs').read_text(),PATCHED)


def without_cargo(real):
    def guarded(args,*rest,**options):
        if args[0]=='cargo':raise AssertionError('cargo ran before the dependency trees were verified')
        return real(args,*rest,**options)
    return guarded


class CandidateSourceGate(SourceTrees):
    def test_candidate_refuses_unknown_source_changes_before_inventory_build_or_evidence(self):
        fixture=self.prepared()
        (fixture.crab/'usb-host/src/lib.rs').write_text('pub fn local() {}\n')
        before=snapshot(fixture.root)
        with mock.patch.object(release,'ROOT',fixture.root),mock.patch.object(subprocess,'run',without_cargo(REAL_RUN)),mock.patch.object(subprocess,'check_output',without_cargo(REAL_OUTPUT)),mock.patch.object(release,'verified_runs',side_effect=AssertionError('evidence accepted')):
            with self.assertRaisesRegex(SystemExit,'Unexpected changes in cached CrabUSB source'):
                release.main(['candidate','--output',str(fixture.root/'out'),'--evidence',str(fixture.root/'run')])
        self.assertEqual(snapshot(fixture.root),before)
        self.assertFalse((fixture.root/'out').exists())

    def test_candidate_rebuilds_from_clean_and_keeps_verified_sources(self):
        fixture=self.prepared()
        calls=[]
        def cargo(change=None):
            def run(args,*rest,**options):
                if args[0]!='cargo':return REAL_RUN(args,*rest,**options)
                calls.append(args[1])
                if change and args[1]=='build':change()
                return subprocess.CompletedProcess(args,0)
            return run
        (fixture.root/'rust-toolchain.toml').write_text('[toolchain]\nchannel = "1.91.1"\n')
        with mock.patch.object(release,'ROOT',fixture.root),mock.patch.dict(os.environ,{'RUSTUP_TOOLCHAIN':'1.91.1'}):
            sources=release.verified_sources()
            with mock.patch.object(subprocess,'run',cargo()):release.build(sources)
            self.assertEqual(calls,['clean','build'])  # #63: never reuse artifacts from before the verification
            with mock.patch.object(subprocess,'run',cargo(lambda:(fixture.dma/'src/lib.rs').write_text('pub fn built_differently() {}\n'))):
                with self.assertRaisesRegex(SystemExit,'Unexpected changes in cached dma-api source'):release.build(sources)

    def test_candidate_refuses_a_toolchain_override_before_cargo_runs(self):
        fixture=self.prepared()
        (fixture.root/'rust-toolchain.toml').write_text('[toolchain]\nchannel = "1.91.1"\n')
        with mock.patch.object(release,'ROOT',fixture.root),mock.patch.dict(os.environ,{'RUSTUP_TOOLCHAIN':'stable'}),mock.patch.object(subprocess,'run',without_cargo(REAL_RUN)):
            sources=release.verified_sources()
            with self.assertRaisesRegex(ValueError,'pinned 1.91.1 toolchain'):release.build(sources)


class ReleaseVersionPolicy(unittest.TestCase):
    def test_matching_current_future_and_prerelease_versions(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            for version in ('0.1.0', '0.2.0', '1.0.0-rc.1', '1.0.0+build.7'):
                with self.subTest(version=version):
                    (root / 'Cargo.toml').write_text(f'[package]\nversion = "{version}"\n')
                    with mock.patch.object(release, 'ROOT', root), mock.patch.object(release, 'command', side_effect=['release/v' + version, '']) as command:
                        self.assertEqual(release.clean_release(), version)
                    self.assertEqual(command.call_args_list, [mock.call('git', '-C', str(root), 'branch', '--show-current'), mock.call('git', '-C', str(root), 'status', '--porcelain')])

    def test_wrong_nonrelease_detached_and_dirty_branches_cannot_build(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            (root / 'Cargo.toml').write_text('[package]\nversion = "0.2.0"\n')
            for branch, status in [('release/v0.1.0', ''), ('feature/main', ''), ('', ''), ('release/v0.2.0', ' M Cargo.toml')]:
                with self.subTest(branch=branch, status=status), mock.patch.object(release, 'ROOT', root), mock.patch.object(release, 'command', side_effect=[branch, status]), mock.patch.object(release, 'build') as build:
                    with self.assertRaisesRegex(ValueError, 'release/v0.2.0'):
                        release.package(root / 'out', [], {}, [], {})
                    build.assert_not_called()
                    self.assertFalse((root / 'out').exists())

    def test_version_must_be_an_explicit_nonempty_string(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            for value in ('""', '" 0.2.0 "', '42', '{ workspace = true }'):
                with self.subTest(value=value):
                    (root / 'Cargo.toml').write_text('[package]\nversion = ' + value + '\n')
                    with self.assertRaisesRegex(ValueError, 'package.version'):
                        release.RELEASE_CONTEXT.package_version(root)


if __name__=='__main__':unittest.main()
