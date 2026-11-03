#!/usr/bin/python3
"""Runs only inside the marked disposable guest; never on the host."""
import errno
import fcntl
import json
import os
import pathlib
import struct
import subprocess
import sys
import traceback

BDF = '0000:00:02.0'
PCI = pathlib.Path('/sys/bus/pci/devices') / BDF
MARKER = pathlib.Path('/sys/firmware/qemu_fw_cfg/by_name/opt/fah-lab/raw')


def report(kind, **values):
    print('FAH_LAB ' + json.dumps({'kind': kind, **values}), flush=True)


def guard():
    expected = pathlib.Path('/run-id').read_bytes()
    assert expected and MARKER.read_bytes() == expected, 'guest marker mismatch'
    assert PCI.joinpath('vendor').read_text().strip() == '0x1b36'
    assert PCI.joinpath('device').read_text().strip() == '0x000d'
    assert PCI.joinpath('class').read_text().strip() == '0x0c0330'
    members = list(PCI.joinpath('iommu_group/devices').iterdir())
    assert [entry.name for entry in members] == [BDF], 'shared/unknown guest group'
    return PCI.joinpath('iommu_group').resolve().name


def bind():
    group = guard()
    assert not PCI.joinpath('driver').exists(), 'expected unused emulated controller'
    PCI.joinpath('driver_override').write_text('vfio-pci')
    pathlib.Path('/sys/bus/pci/drivers/vfio-pci/bind').write_text(BDF)
    assert PCI.joinpath('driver').resolve().name == 'vfio-pci'
    return group


def ioctl_buffer(fd, code, size, *fields):
    data = bytearray(size)
    struct.pack_into('<' + 'I' * len(fields), data, 0, *fields)
    fcntl.ioctl(fd, code, data, True)
    return data


def metadata():
    group_number = bind()
    with open('/dev/vfio/vfio', 'r+b', buffering=0) as container, open('/dev/vfio/' + group_number, 'r+b', buffering=0) as group:
        assert fcntl.ioctl(container, 0x3b64) == 0
        assert fcntl.ioctl(container, 0x3b65, 1) > 0, 'translated type1 IOMMU required'
        status = ioctl_buffer(group, 0x3b67, 8, 8, 0)
        assert struct.unpack_from('<II', status)[1] & 1
        fcntl.ioctl(group, 0x3b68, struct.pack('<i', container.fileno()))
        fcntl.ioctl(container, 0x3b66, 1)
        device = fcntl.ioctl(group, 0x3b6a, bytearray(BDF.encode() + b'\0'), True)
        assert isinstance(device, int) and device >= 0
        try:
            info = ioctl_buffer(device, 0x3b6b, 32, 32, 0)
            _, flags, regions, irqs = struct.unpack_from('<IIII', info)
            region_info = []
            for index in range(regions):
                try:
                    data = ioctl_buffer(device, 0x3b6c, 4096, 4096, 0, index, 0)
                except OSError as error:
                    if index == 8 and error.errno == errno.EINVAL:
                        region_info.append(dict(index=index, available=False, errno=error.errno, reason='non-VGA PCI device'))
                        continue
                    raise
                size, region_flags, _, cap_offset, length, offset = struct.unpack_from('<IIIIQQ', data)
                region_info.append(dict(index=index, argsz=size, flags=region_flags, cap_offset=cap_offset, size=length, offset=offset, raw=data[:size].hex()))
            irq_info = []
            for index in range(irqs):
                data = ioctl_buffer(device, 0x3b6d, 16, 16, 0, index, 0)
                argsz, irq_flags, _, count = struct.unpack('<IIII', data)
                irq_info.append(dict(index=index, argsz=argsz, flags=irq_flags, count=count))
            iommu = ioctl_buffer(container, 0x3b70, 4096, 4096, 0)
            report('metadata', bdf=BDF, group=group_number, viable=True, translated=True, flags=flags, regions=region_info, irqs=irq_info, iommu_raw=iommu[:struct.unpack_from('<I', iommu)[0]].hex(), kernel=os.uname().release, cmdline=pathlib.Path('/proc/cmdline').read_text())
        finally:
            os.close(device)
    return group_number


def restore():
    guard()
    assert PCI.joinpath('driver').resolve().name=='vfio-pci'
    pathlib.Path('/sys/bus/pci/drivers/vfio-pci/unbind').write_text(BDF)
    PCI.joinpath('driver_override').write_text('xhci_hcd')
    for module in json.loads(pathlib.Path('/restore-modules.json').read_text()):
        subprocess.run(['/bin/insmod',module],check=True)
    if not PCI.joinpath('driver').exists():
        pathlib.Path('/sys/bus/pci/drivers/xhci_hcd/bind').write_text(BDF)
    assert PCI.joinpath('driver').resolve().name=='xhci_hcd'
    import time
    deadline=time.monotonic()+10
    disk=pathlib.Path('/dev/sda')
    while not disk.exists() and time.monotonic()<deadline:time.sleep(.05)
    with disk.open('rb')as device:
        data=device.read(512)
    assert data==bytes(range(256))*2,'synthetic sector mismatch after guest restoration'
    report('restored',driver='xhci_hcd',disk='/dev/sda',bytes=len(data),data_correct=True)


def main():
    report('start', suite=sys.argv[1], run_id=pathlib.Path('/run-id').read_text())
    metadata()
    if sys.argv[1]=='fixture':
        restore()
    else:
        assert sys.argv[1] == 'feasibility', 'unknown suite'
    report('result', success=True, suite=sys.argv[1], evidence='emulated_model_only')


if __name__ == '__main__':
    try:
        main()
    except BaseException as error:
        report('failure', error=repr(error), traceback=traceback.format_exc())
        report('result', success=False, suite=sys.argv[1])
        sys.exit(1)
