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
