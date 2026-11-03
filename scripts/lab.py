#!/usr/bin/env python3
"""Disk-backed, emulated-only, release-branch QEMU lab. No host PCI mutations."""
import argparse
import fcntl
import gzip
import hashlib
import json
import lzma
import os
import pathlib
import re
import selectors
import shutil
import socket
import struct
import subprocess
import sys
import time

ROOT = pathlib.Path(__file__).resolve().parents[1]
KERNEL = '6.12.95+deb13-amd64'
MACHINE = 'pc-q35-10.0'
DEFAULT_LAB = pathlib.Path.home() / '.cache/fah-lab'


def command(*args):
    return subprocess.check_output(args, text=True).strip()


def digest(path):
    return hashlib.sha256(pathlib.Path(path).read_bytes()).hexdigest()


def release_only():
    branch = command('git', '-C', str(ROOT), 'branch', '--show-current')
    if branch != 'release/v0.1.0':
        raise ValueError('VM preparation/execution requires release/v0.1.0')
    if command('git', '-C', str(ROOT), 'status', '--porcelain'):
        raise ValueError('commit changes before VM preparation/execution')


def safe_lab(path):
    path = pathlib.Path(path).expanduser().absolute()
    if path == pathlib.Path('/') or path.is_symlink() or path.is_relative_to('/tmp') or path == ROOT:
        raise ValueError('lab must be a dedicated disk directory outside /tmp')
    ancestor = path
    while not ancestor.exists():
        ancestor = ancestor.parent
    fs = command('findmnt', '-no', 'FSTYPE', '--target', str(ancestor))
    if fs in ('tmpfs', 'ramfs', 'devtmpfs'):
        raise ValueError('VM artifacts must reside on disk, not tmpfs')
    return path


def inventory(lab):
    memory = {line.split(':')[0]: int(line.split()[1]) for line in pathlib.Path('/proc/meminfo').read_text().splitlines()}
    ancestor = lab
    while not ancestor.exists():
        ancestor = ancestor.parent
    with open('/dev/kvm', 'r+b', buffering=0) as kvm:
        assert fcntl.ioctl(kvm, 0xae00) == 12
    if memory['MemAvailable'] < 4 * 1024 * 1024:
        raise ValueError('less than4GiB MemAvailable; defer the2GiB guest')
    if shutil.disk_usage(ancestor).free < 2 * 1024**3:
        raise ValueError('less than2GiB free disk')
    return dict(memory_kib=memory, pressure=pathlib.Path('/proc/pressure/memory').read_text(), disk_free=shutil.disk_usage(ancestor).free, qemu=command('qemu-system-x86_64', '--version').splitlines()[0], kernel=KERNEL, machine=MACHINE, ram_mib=2048, vcpus=2, artifacts=str(lab), kvm=True, physical_passthrough=False)


def cpio(root):
    """Deterministic newc archive; input files and symlinks only, no device nodes."""
    archive = bytearray()
    for index, path in enumerate([root] + sorted(root.rglob('*')), 1):
        name = '.' if path == root else str(path.relative_to(root))
        encoded = name.encode() + b'\0'
        data = os.readlink(path).encode() if path.is_symlink() else path.read_bytes() if path.is_file() else b''
        mode = path.lstat().st_mode
        fields = [index, mode, 0, 0, 1, 0, len(data), 0, 0, 0, 0, len(encoded), 0]
        archive += b'070701' + ''.join(f'{value:08x}' for value in fields).encode() + encoded
        archive += b'\0' * (-len(archive) % 4)
        archive += data
        archive += b'\0' * (-len(archive) % 4)
    name = b'TRAILER!!!\0'
    fields = [0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, len(name), 0]
    archive += b'070701' + ''.join(f'{value:08x}' for value in fields).encode() + name
    archive += b'\0' * (-len(archive) % 4)
    return bytes(archive)


def prepare(lab):
    release_only()
    info = inventory(lab)
    lab.mkdir(parents=True, exist_ok=True)
    stage = lab / 'rootfs'
    if stage.is_symlink():
        raise ValueError('refusing symlink rootfs')
    if stage.exists():
        if not (stage / '.fah-generated').exists():
            raise ValueError('refusing to replace unmarked rootfs')
        shutil.rmtree(stage)
    stage.mkdir()
    (stage / '.fah-generated').touch()
    for name in ['bin', 'dev', 'proc', 'sys', 'tmp', 'run', 'modules', 'root']:
        (stage / name).mkdir()
    sources = {}

    def copy_file(source, target=None):
        source = pathlib.Path(source)
        dest = stage / (str(target) if target else str(source).lstrip('/'))
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, dest)
        sources[str(source)] = digest(source)

    def executable(source, target=None):
        copy_file(source, target)
        for line in command('ldd', str(source)).splitlines():
            match = re.search(r'(/[^ ]+)', line)
            if match and pathlib.Path(match[1]).is_file():
                copy_file(match[1])

    executable('/usr/bin/busybox', 'bin/busybox')
    for applet in ['sh', 'mount', 'insmod', 'poweroff', 'stty', 'sleep', 'cat', 'echo', 'mkdir', 'ls', 'dmesg']:
        (stage / 'bin' / applet).symlink_to('/bin/busybox')
    executable('/usr/bin/python3.13', 'bin/python3')
    library = pathlib.Path('/usr/lib/python3.13')
    for path in library.rglob('*'):
        if path.is_file() and '__pycache__' not in path.parts and 'site-packages' not in path.parts:
            if path.suffix == '.so':
                executable(path)
            else:
                copy_file(path)
    module_paths = []
    for module in ['vfio-pci', 'qemu_fw_cfg']:
        for line in command('/sbin/modprobe', '--show-depends', '-S', KERNEL, module).splitlines():
            if line.startswith('insmod '):
                path = pathlib.Path(line.split()[1])
                if path not in module_paths:
                    module_paths.append(path)
    modules = []
    for index, path in enumerate(module_paths):
        target = stage / 'modules' / f'{index:02}.ko'
        target.write_bytes(lzma.decompress(path.read_bytes()) if path.suffix == '.xz' else path.read_bytes())
        sources[str(path)] = digest(path)
        modules.append('/modules/' + target.name)
    kernel = pathlib.Path('/boot/vmlinuz-' + KERNEL)
    config = pathlib.Path('/boot/config-' + KERNEL)
    expected = json.loads((ROOT / 'scripts/lab/kernel.json').read_text())
    assert digest(kernel) == expected['kernel_sha256'], 'kernel checksum mismatch'
    assert digest(config) == expected['config_sha256'], 'kernel config checksum mismatch'
    shutil.copy2(kernel, lab / 'kernel')
    sources[str(kernel)] = digest(kernel)
    sources[str(config)] = digest(config)
    run_id = 'fah-' + command('git', '-C', str(ROOT), 'rev-parse', '--short=12', 'HEAD')
    (stage / 'run-id').write_text(run_id)
    copy_file(ROOT / 'scripts/lab/guest.py', 'guest.py')
    init = '''#!/bin/sh
export PATH=/bin
mount -t proc proc /proc
mount -t sysfs sysfs /sys
mount -t devtmpfs devtmpfs /dev
mount -t tmpfs tmpfs /tmp
mount -t tmpfs tmpfs /run
stty raw -echo < /dev/ttyS0
'''
    init += ''.join(f'insmod {module} || echo FAH_MODULE_FAILED_{module}\n' for module in modules)
    init += '/bin/python3 -u /guest.py "$(cat /proc/cmdline | /bin/busybox sed -n \'s/.*fah_suite=\\([^ ]*\\).*/\\1/p\')"\necho FAH_GUEST_DONE\npoweroff -f\nwhile true; do sleep 1; done\n'
    (stage / 'init').write_text(init)
    (stage / 'init').chmod(0o755)
    (lab / 'initramfs.gz').write_bytes(gzip.compress(cpio(stage), mtime=0))
    info.update(run_id=run_id, commit=command('git', '-C', str(ROOT), 'rev-parse', 'HEAD'), sources=sources, initramfs_sha256=digest(lab / 'initramfs.gz'), kernel_sha256=digest(kernel))
    (lab / 'manifest.json').write_text(json.dumps(info, indent=2))
    print(json.dumps({'prepared': str(lab), 'run_id': run_id, 'initramfs_bytes': (lab / 'initramfs.gz').stat().st_size}))


def run(lab, suite):
    release_only()
    inventory(lab)
    manifest = json.loads((lab / 'manifest.json').read_text())
    assert digest(lab / 'initramfs.gz') == manifest['initramfs_sha256'], 'initramfs checksum mismatch'
    assert digest(lab / 'kernel') == manifest['kernel_sha256'], 'kernel checksum mismatch'
    assert manifest['commit'] == command('git', '-C', str(ROOT), 'rev-parse', 'HEAD'), 'rebuild initramfs for current release commit'
    run_dir = lab / 'runs' / f'{time.time_ns()}-{suite}'
    run_dir.mkdir(parents=True)
    (run_dir / '.fah-lab-run').touch()
    shutil.copy2(lab / 'manifest.json', run_dir / 'manifest.json')
    args = ['qemu-system-x86_64', '-machine', MACHINE + ',accel=kvm,kernel-irqchip=split', '-cpu', 'host', '-m', '2048', '-smp', '2', '-nodefaults', '-display', 'none', '-no-reboot', '-serial', 'stdio', '-monitor', 'none', '-kernel', str(lab / 'kernel'), '-initrd', str(lab / 'initramfs.gz'), '-append', 'console=ttyS0 rdinit=/init intel_iommu=on iommu.strict=1 panic=-1 fah_suite=' + suite, '-fw_cfg', 'name=opt/fah-lab,string=' + manifest['run_id'], '-device', 'intel-iommu,intremap=on,caching-mode=on,device-iotlb=on', '-device', 'qemu-xhci,id=xhci,addr=02.0,p2=4,p3=4']
    (run_dir / 'command.json').write_text(json.dumps(args, indent=2))
    result = False
    events = []
    with (run_dir / 'qemu.stderr').open('wb') as errors, (run_dir / 'serial.log').open('wb') as serial:
        process = subprocess.Popen(args, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=errors)
        try:
            selector = selectors.DefaultSelector()
            selector.register(process.stdout, selectors.EVENT_READ)
            deadline = time.monotonic() + 180
            pending = b''
            while time.monotonic() < deadline:
                readable = selector.select(1)
                if not readable:
                    if process.poll() is not None:
                        break
                    continue
                data = os.read(process.stdout.fileno(), 65536)
                if not data:
                    break
                serial.write(data)
                serial.flush()
                pending += data
                while b'\n' in pending:
                    line, pending = pending.split(b'\n', 1)
                    if line.startswith(b'FAH_LAB '):
                        event = json.loads(line[8:])
                        events.append(event)
                        print(json.dumps({key:value for key,value in event.items() if key not in ("regions","iommu_raw")}), flush=True)
                        if event['kind'] == 'result':
                            result = event['success']
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=5)
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
    (run_dir / 'events.json').write_text(json.dumps(events, indent=2))
    (run_dir / 'result.json').write_text(json.dumps({'success': result, 'qemu_exit': process.returncode, 'suite': suite, 'evidence': 'emulated_only'}))
    print(json.dumps({'artifacts': str(run_dir), 'success': result}))
    return 0 if result and process.returncode == 0 else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['plan', 'prepare', 'run'])
    parser.add_argument('--lab', type=pathlib.Path, default=DEFAULT_LAB)
    parser.add_argument('--suite', choices=['feasibility'], default='feasibility')
    args = parser.parse_args()
    lab = safe_lab(args.lab)
    if args.action == 'plan':
        print(json.dumps(inventory(lab), indent=2))
    elif args.action == 'prepare':
        prepare(lab)
    else:
        return run(lab, args.suite)
    return 0


if __name__ == '__main__':
    try:
        sys.exit(main())
    except Exception as error:
        print(f'lab error: {error}', file=sys.stderr)
        sys.exit(1)
