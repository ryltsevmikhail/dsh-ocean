#!/usr/bin/env python3
"""CI-only pinned SDK fixture environment; does not install or modify a runtime."""
import argparse
import json
from pathlib import Path
import platform
import re


def environment(manifest, architecture, root):
    archive = {'x86_64': 'linux-x64', 'aarch64': 'linux-arm64'}.get(architecture)
    if archive is None:
        raise ValueError('unsupported native CI architecture')
    root = Path(root)
    if not root.is_absolute() or root == Path('/') or any(ord(c) < 32 or ord(c) == 127 for c in str(root)):
        raise ValueError('fixture root must be an absolute single-line path')
    if manifest['schema'] != 1 or manifest['dsh']['package'] != '@deepseek-ai/dsh':
        raise ValueError('unsupported runtime manifest')
    for target in ('linux-x64', 'linux-arm64'):
        if not re.fullmatch('[0-9a-f]{64}', manifest['node']['archives'][target]['sha256']):
            raise ValueError('both native Node archives require SHA256 pins')
    versions = [manifest['node']['version'], manifest['dsh']['version'], manifest['pnpm']['version']]
    if not all(re.fullmatch(r'[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?', version) for version in versions):
        raise ValueError('exact runtime versions required')
    current = root / 'ocean-data/current'
    modules = current / 'harness/node_modules/@deepseek-ai'
    return {
        'DSH_NODE_VERSION': versions[0],
        'DSH_NODE_SHA256': manifest['node']['archives'][archive]['sha256'],
        'DSH_VERSION': versions[1],
        'DSH_PNPM_VERSION': versions[2],
        'DSH_OCEAN_SDK_FIXTURE': str(current),
        'DSH_LOCALE_CLIENT_SOURCE': str(modules / 'dsh-client-locale/lib/client.js'),
        'DSH_CLIENT_MODULES_SOURCE': str(modules / 'dsh-client-modules/lib/client.js'),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--manifest', type=Path, default=Path(__file__).resolve().parents[1] / 'runtime-manifest.json')
    parser.add_argument('--arch', default=platform.machine())
    parser.add_argument('--fixture-root', required=True)
    args = parser.parse_args()
    values = environment(json.loads(args.manifest.read_text()), args.arch, args.fixture_root)
    # Validate everything before emitting anything into GITHUB_ENV.
    print(''.join(f'{key}={value}\n' for key, value in values.items()), end='')


if __name__ == '__main__':
    main()
