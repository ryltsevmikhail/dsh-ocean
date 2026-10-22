#!/usr/bin/env python3
"""Build/verify the optional locale asset, separate from signed shell/runtime feeds."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tarfile

ROOT = Path(__file__).resolve().parents[1]
PACKAGE = ROOT / 'language-pack'
METADATA = 'language-pack-artifact.json'


def verify(directory):
    package = json.loads((PACKAGE / 'package.json').read_text())
    meta = json.loads((directory / METADATA).read_text())
    name = f'dsh-ocean-desktop-language-pack-{package["version"]}.tgz'
    assert meta['filename'] == name and meta['version'] == package['version'], 'unexpected locale asset'
    archive = directory / name
    assert archive.is_file() and not archive.is_symlink(), 'missing regular locale asset'
    data = archive.read_bytes()
    assert len(data) == meta['size'] and hashlib.sha256(data).hexdigest() == meta['sha256'], 'locale checksum mismatch'
    expected = {'package/package.json', 'package/cordis.patch.yml', 'package/README.md',
                'package/LICENSE', 'package/lib/client.js', 'package/lib/index.js',
                'package/data/inventory.json', 'package/data/unchanged.json'}
    with tarfile.open(archive, 'r:gz') as tar:
        members = tar.getmembers()
        assert {m.name for m in members} == expected and len(members) == len(expected), 'unexpected packed paths'
        assert all(m.isfile() and m.size < 4 * 1024 * 1024 for m in members), 'unsafe locale member'
        packed = json.load(tar.extractfile('package/package.json'))
        assert packed == package and not any(k in packed.get('scripts', {}) for k in
            ('preinstall', 'install', 'postinstall', 'prepare')), 'unexpected package manifest/scripts'
    return meta


def build(directory):
    directory.mkdir(parents=True, exist_ok=True)
    assert not (directory / METADATA).exists(), 'refusing to replace locale metadata'
    subprocess.run(['node', 'scripts/build.mjs'], cwd=PACKAGE, check=True)
    result = subprocess.run(['npm', 'pack', '--ignore-scripts', '--json', '--pack-destination', str(directory)],
                            cwd=PACKAGE, check=True, capture_output=True, text=True)
    entries = json.loads(result.stdout)
    if isinstance(entries, dict):
        entries = list(entries.values())  # npm supports array and keyed-object JSON.
    assert len(entries) == 1
    entry = entries[0]
    version = json.loads((PACKAGE / 'package.json').read_text())['version']
    name = f'dsh-ocean-desktop-language-pack-{version}.tgz'
    assert entry['filename'] == name, 'unexpected npm pack filename'
    data = (directory / name).read_bytes()
    (directory / METADATA).write_text(json.dumps({'filename': name, 'version': version,
        'size': len(data), 'sha256': hashlib.sha256(data).hexdigest()}, indent=2) + '\n')
    return verify(directory)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument('--output-dir', type=Path)
    mode.add_argument('--verify', type=Path)
    args = parser.parse_args()
    print(json.dumps(verify(args.verify.resolve()) if args.verify else build(args.output_dir.resolve()), indent=2))
