"""Shared version and branch contract for the lab and release tools."""
import tomllib

M8_CASES = {'capabilities', 'dma_large', 'scatter_read10', 'bei_drain', 'literal_noop', 'unknown_retention', 'session_handles'}


def package_version(root):
    version = tomllib.loads((root / 'Cargo.toml').read_text())['package']['version']
    if not isinstance(version, str) or not version or version.strip() != version:
        raise ValueError('release tools require an explicit Cargo package.version string')
    return version


def require_clean_release(root, command):
    version = package_version(root)
    expected = 'release/v' + version
    branch = command('git', '-C', str(root), 'branch', '--show-current')
    if branch != expected:
        raise ValueError(f'release operation requires {expected}; current branch is {branch or "detached HEAD"}')
    if command('git', '-C', str(root), 'status', '--porcelain'):
        raise ValueError(f'release operation requires clean {expected}; commit changes first')
    return version

TOPOLOGY_CASES = {'hub_route', 'isochronous', 'stream_inquiry'}
