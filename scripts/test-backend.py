#!/usr/bin/env python3
"""Exercise the maintained backend's private tests using the patched DMA crate."""
import pathlib
import subprocess
import shutil
root = pathlib.Path(__file__).resolve().parents[1]
shutil.copyfile(root / 'patches/crabusb-unit.Cargo.lock', root / '.fah-cache/CrabUSB/Cargo.lock')
subprocess.run(['cargo', 'test', '--locked', '--manifest-path', str(root / '.fah-cache/CrabUSB/usb-host/Cargo.toml'),
                '--no-default-features', '--features', 'hosted-direct,dma-api/fah-vfio', '--lib',
                '--config', 'patch.crates-io.dma-api.path="' + str(root / '.fah-cache/dma-api') + '"'],check=True)
