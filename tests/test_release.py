"""Release policy regressions; these tests never start a VM or access hardware."""
import hashlib
import importlib.util
import json
import pathlib
import tempfile
import unittest

ROOT=pathlib.Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location('fah_release',ROOT/'scripts/release.py')
release=importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


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
                if suite=='explicit':(path/'trace-validation.json').write_text(json.dumps([dict(verified=True)]*11))
                if suite=='faults':(path/'failure-matrix.json').write_text(json.dumps([dict(verified=True)]*11))
            self.assertEqual(len(release.verified_runs(paths,'rev','bin')),6)
            for candidates,commit,binary in [(paths,'old','bin'),(paths,'rev','changed'),(paths[:-1],'rev','bin')]:
                with self.assertRaises(ValueError):release.verified_runs(candidates,commit,binary)
            with self.assertRaises(ValueError):release.verified_runs(paths+[paths[0]],'rev','bin')


if __name__=='__main__':unittest.main()
