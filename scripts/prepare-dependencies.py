#!/usr/bin/env python3
"""Prepare owned, pinned sources. Refuse unknown changes instead of overwriting them."""
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
    if git('status','--porcelain',capture=True).stdout.strip():
        raise SystemExit('Unexpected source changes; refusing to patch.')
    git('apply','--check',str(PATCH))
    git('apply',str(PATCH))
if git('diff', '--binary', 'HEAD', capture=True).stdout != PATCH.read_text():
    raise SystemExit('Unexpected modifications beyond the maintained patch.')
if git('ls-files', '--others', '--exclude-standard', capture=True).stdout.strip():
    raise SystemExit('Unexpected untracked dependency files.')
print('Pinned CrabUSB hosted backend is ready:',REV)
