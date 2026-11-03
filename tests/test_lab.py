"""No-VM regressions for guest-only lab tooling."""
import errno
import importlib.util
import pathlib
import struct
import unittest
from unittest.mock import patch, MagicMock

ROOT = pathlib.Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('fah_guest', ROOT / 'scripts/lab/guest.py')
guest = importlib.util.module_from_spec(spec)
spec.loader.exec_module(guest)


class GuestMetadata(unittest.TestCase):
    def run_probe(self, bad_index=8, error=errno.EINVAL):
        reports = []

        def ioctl(_fd, code, arg=0, *_args):
            if code == 0x3b64:
                return 0
            if code in (0x3b65, 0x3b6a):
                return 1 if code == 0x3b65 else 100
            if code == 0x3b67:
                struct.pack_into('<II', arg, 0, 8, 1)
            elif code == 0x3b6b:
                struct.pack_into('<IIII', arg, 0, 32, 2, 9, 0)
            elif code == 0x3b6c:
                index = struct.unpack_from('<I', arg, 8)[0]
                if index == bad_index:
                    raise OSError(error, 'injected')
            return 0

        file = MagicMock()
        file.__enter__.return_value.fileno.return_value = 3
        with patch.object(guest, 'bind', return_value='1'), patch.object(guest.fcntl, 'ioctl', side_effect=ioctl), patch('builtins.open', return_value=file), patch.object(guest.os, 'close'), patch.object(guest, 'report', side_effect=lambda kind, **data: reports.append((kind, data))), patch.object(pathlib.Path, 'read_text', return_value='guest'):
            guest.metadata()
        return reports

    def test_absent_vga_is_explicitly_unavailable(self):
        reports = self.run_probe()
        regions = reports[0][1]['regions']
        self.assertEqual(len(regions), 9)
        self.assertEqual(regions[8]['errno'], errno.EINVAL)
        self.assertFalse(regions[8]['available'])
        self.assertEqual(regions[0]['index'], 0)

    def test_other_errors_are_not_silenced(self):
        for index, error in [(0, errno.EINVAL), (8, errno.EIO)]:
            with self.assertRaises(OSError):
                self.run_probe(index, error)


if __name__ == '__main__':
    unittest.main()

host_spec = importlib.util.spec_from_file_location('fah_lab', ROOT / 'scripts/lab.py')
lab = importlib.util.module_from_spec(host_spec)
host_spec.loader.exec_module(lab)


class LabGuards(unittest.TestCase):
    def test_branch_and_tmpfs_guards(self):
        with patch.object(lab, 'command', return_value='feature/not-release'):
            with self.assertRaises(ValueError):
                lab.release_only()
        with self.assertRaises(ValueError):
            lab.safe_lab('/tmp/fah-lab')
        with patch.object(lab, 'command', return_value='tmpfs'):
            with self.assertRaises(ValueError):
                lab.safe_lab('/home/example/fah-lab')

    def test_missing_and_corrupt_images_cannot_launch_qemu(self):
        import tempfile
        import json
        with tempfile.TemporaryDirectory() as directory, patch.object(lab, 'release_only'), patch.object(lab, 'inventory'), patch.object(lab.subprocess, 'Popen') as popen:
            path = pathlib.Path(directory)
            with self.assertRaises(FileNotFoundError):
                lab.run(path, 'fixture')
            (path / 'manifest.json').write_text(json.dumps({'initramfs_sha256': 'bad'}))
            (path / 'initramfs.gz').write_bytes(b'corrupt')
            with self.assertRaises(AssertionError):
                lab.run(path, 'fixture')
            popen.assert_not_called()

    def test_missing_kvm_and_low_memory_are_explicit_failures(self):
        with patch.object(pathlib.Path, 'read_text', return_value='MemAvailable: 10000000 kB\n'), patch('builtins.open', side_effect=FileNotFoundError('/dev/kvm')):
            with self.assertRaises(FileNotFoundError):
                lab.inventory(ROOT)
        file = MagicMock()
        with patch.object(pathlib.Path, 'read_text', return_value='MemAvailable: 1000 kB\n'), patch('builtins.open', return_value=file), patch.object(lab.fcntl, 'ioctl', return_value=12):
            with self.assertRaisesRegex(ValueError, 'MemAvailable'):
                lab.inventory(ROOT)

    def test_cleanup_refuses_unknown_state_and_pid_reuse(self):
        import tempfile
        import os
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory)
            for name in ['..', '../outside', '', '/etc']:
                with self.assertRaises(ValueError):
                    lab.selected_run(path, name)
            run = path / 'runs/test'
            run.mkdir(parents=True)
            with self.assertRaises(ValueError):
                lab.selected_run(path, 'test')
            (run / '.fah-lab-run').touch()
            (run / 'pid').write_text(str(os.getpid()))
            with self.assertRaises(ValueError):
                lab.manage_run(path, 'test', 'stop')
            self.assertTrue(run.exists())
            (run / 'pid').write_text('999999999')
            lab.manage_run(path, 'test', 'cleanup')
            self.assertFalse(run.exists())


class FixtureClassification(unittest.TestCase):
    def test_capabilities_identify_high_speed_keyboard_without_overwriting_storage(self):
        keyboard=[dict(transfer_type='interrupt',direction='in',address=129)]
        storage=[dict(transfer_type='bulk',direction='in',address=129),dict(transfer_type='bulk',direction='out',address=2)]
        self.assertEqual(guest.fixture_role(keyboard),'keyboard')
        self.assertEqual(guest.fixture_role(storage),'storage')
        with self.assertRaises(ValueError):guest.fixture_role([])
        with self.assertRaises(ValueError):guest.fixture_role(keyboard+storage)
