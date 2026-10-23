#!/usr/bin/env python3
"""Check exact compiled-in localization bytes in one native ELF; not release clearance."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import stat

ROOT = Path(__file__).resolve().parents[1]
LANGUAGES = ('en', 'ru', 'es', 'pt-BR', 'de', 'fr', 'ja', 'ko')
DSH = ('LICENSE', 'builtin-locale-preflight.mjs', 'builtin-locale-sdk-preflight.mjs',
       'lib/client.js', 'lib/index.js', 'overlay.yml', 'package.json')


def inspect(binary, architecture):
    machine = {'x86_64': 62, 'aarch64': 183}.get(architecture)
    if machine is None:
        raise ValueError('unsupported native ELF architecture')
    descriptor = os.open(binary, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(descriptor, 'rb') as source:
        metadata = os.fstat(source.fileno())
        if not stat.S_ISREG(metadata.st_mode) or not 64 <= metadata.st_size <= 512 * 1024 * 1024:
            raise ValueError('bounded regular ELF required')
        contents = source.read(512 * 1024 * 1024 + 1)
    if len(contents) != metadata.st_size:
        raise ValueError('ELF changed during inspection')
    if (contents[:7] != b'\x7fELF\x02\x01\x01'
            or int.from_bytes(contents[16:18], 'little') not in (2, 3)
            or int.from_bytes(contents[18:20], 'little') != machine):
        raise ValueError('wrong ELF class, byte order, kind or native machine')
    paths = [f'locales/ocean/{language}.json' for language in LANGUAGES]
    paths += [f'locales/dsh/{name}' for name in DSH]
    resources = []
    for relative in paths:
        payload = (ROOT / relative).read_bytes()
        if not payload or payload not in contents:
            raise ValueError(f'missing exact embedded localization resource: {relative}')
        resources.append({'path': relative, 'bytes': len(payload),
                          'sha256': hashlib.sha256(payload).hexdigest()})
    return {'schema': 1, 'architecture': architecture, 'elfBytes': len(contents),
            'elfSha256': hashlib.sha256(contents).hexdigest(), 'resources': resources,
            'releaseApproval': False,
            'scope': 'Exact resource bytes and native ELF architecture only; not execution, full license closure, signature or release approval.'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--arch', default=platform.machine())
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    report = json.dumps(inspect(args.binary, args.arch), ensure_ascii=False, indent=2) + '\n'
    if args.output:
        with args.output.open('x', encoding='utf-8') as destination:
            destination.write(report)
    print(report, end='')


if __name__ == '__main__':
    main()
