#!/usr/bin/env python3
"""Exercise the maintained backend's private tests using the patched DMA crate."""
import pathlib
import subprocess
root = pathlib.Path(__file__).resolve().parents[1]
subprocess.run(['cargo', 'test', '--manifest-path', str(root / '.fah-cache/CrabUSB/usb-host/Cargo.toml'),
                '--no-default-features', '--features', 'hosted-direct,dma-api/fah-vfio', '--lib',
                '--config', 'patch.crates-io.dma-api.path="' + str(root / '.fah-cache/dma-api') + '"'],check=True)
