#!/usr/bin/env python3
"""Prepare owned, pinned sources. Refuse unknown changes instead of overwriting them."""
import hashlib
import pathlib
import subprocess

ROOT = pathlib.Path(__file__).resolve().parents[1]
REV = 'e83b56b0672460b400105f3855222af58c03af87'
DEST = ROOT / '.fah-cache' / 'CrabUSB'
PATCH = ROOT / 'patches' / 'crabusb-hosted-backend.patch'

def git(*args, capture=False):
    return subprocess.run(['git', '-C', str(DEST), *args], check=True, text=True,
                          stdout=subprocess.PIPE if capture else None)

if not PATCH.is_file():
    raise SystemExit('Missing dependency patch; no source changes were made.')

if not DEST.exists():
    DEST.parent.mkdir(exist_ok=True)
    subprocess.run(['git','clone','--no-checkout','https://github.com/drivercraft/CrabUSB.git',str(DEST)],check=True)
    git('checkout','--detach',REV)
if git('rev-parse','HEAD',capture=True).stdout.strip() != REV:
    raise SystemExit('Unexpected source revision; refusing to patch.')
result = subprocess.run(['git','-C',str(DEST),'apply','--reverse','--check',str(PATCH)],capture_output=True)
if result.returncode:
    diff = git('diff', '--binary', 'HEAD', capture=True).stdout
    if git('ls-files', '--others', '--exclude-standard', capture=True).stdout.strip():
        raise SystemExit('Unexpected untracked dependency files.')
    if diff:
        known = (ROOT / 'patches' / 'crabusb-previous.sha256').read_text().splitlines()
        if hashlib.sha256(diff.encode()).hexdigest() not in known:
            raise SystemExit('Unexpected source changes; refusing to patch.')
        subprocess.run(['git', '-C', str(DEST), 'apply', '--reverse', '-'], input=diff, text=True, check=True)
    git('apply','--check',str(PATCH))
    git('apply',str(PATCH))
if git('diff', '--binary', 'HEAD', capture=True).stdout != PATCH.read_text():
    raise SystemExit('Unexpected modifications beyond the maintained patch.')
if git('ls-files', '--others', '--exclude-standard', capture=True).stdout.strip():
    raise SystemExit('Unexpected untracked dependency files.')
print('Pinned CrabUSB hosted backend is ready:',REV)
# Maintained dma-api source is unpacked only from the checksum-pinned registry archive.
import io
import json
import shutil
import tarfile
import tempfile
import urllib.request
DMA_SHA = '6ce802d0c429c6966bea151e8e78eb5906d5b5e2cdebd0e960097cbe3654a3db'
DMA_DEST = ROOT / '.fah-cache' / 'dma-api'
DMA_STAMP = ROOT / '.fah-cache' / 'dma-api-tree.json'
DMA_PATCH = ROOT / 'patches' / 'dma-api-vfio.patch'
def tree(directory):
    return {str(p.relative_to(directory)): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in directory.rglob('*') if p.is_file()}
archive = ROOT / '.fah-cache' / 'dma-api-0.7.3.crate'
if not archive.exists():
    with urllib.request.urlopen('https://static.crates.io/crates/dma-api/dma-api-0.7.3.crate', timeout=60) as response:
        payload = response.read()
    if hashlib.sha256(payload).hexdigest() != DMA_SHA:
        raise SystemExit('DMA archive checksum mismatch')
    archive.write_bytes(payload)
payload = archive.read_bytes()
if hashlib.sha256(payload).hexdigest() != DMA_SHA:
    raise SystemExit('Cached DMA archive checksum mismatch')
with tempfile.TemporaryDirectory(dir=ROOT / '.fah-cache') as temporary:
    with tarfile.open(fileobj=io.BytesIO(payload)) as package:
        package.extractall(temporary, filter='data')
    expected = pathlib.Path(temporary) / 'dma-api-0.7.3'
    subprocess.run(['git', 'apply', '--unsafe-paths', '--directory', str(expected), str(DMA_PATCH)], check=True)
    expected_tree = tree(expected)
    if DMA_DEST.exists() and tree(DMA_DEST) != expected_tree:
        if not DMA_STAMP.exists() or tree(DMA_DEST) != json.loads(DMA_STAMP.read_text()):
            raise SystemExit('Unexpected changes in cached dma-api source')
        shutil.rmtree(DMA_DEST)
    if not DMA_DEST.exists(): shutil.copytree(expected, DMA_DEST)
    DMA_STAMP.write_text(json.dumps(expected_tree, sort_keys=True))
print('Pinned dma-api VFIO access patch is ready:', DMA_SHA)
