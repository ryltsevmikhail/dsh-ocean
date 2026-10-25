#!/usr/bin/env python3
"""Bind a prebuilt release input to the reviewed lock and build commit.

No signing, downloading, extraction, trust installation or publication here.
Workflow commit and product build commit are deliberately separate identities.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import stat


def validate(lock, directory, tag_commit):
    if lock.get('product') not in ('shell', 'runtime') or lock.get('channel') != 'next' or lock.get('target') != 'linux-x86_64':
        raise ValueError('unsupported product/channel/target')
    if not re.fullmatch('[a-f0-9]{40}', lock.get('sourceCommit', '')) or tag_commit != lock['sourceCommit']:
        raise ValueError('release tag must resolve to the accepted build commit')
    name = lock.get('filename', '')
    if not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9._-]*\.tar\.gz', name):
        raise ValueError('archive must be a basename')
    archive = Path(directory) / name
    try:
        info = archive.lstat()
    except FileNotFoundError as exc:
        raise ValueError('missing accepted archive') from exc
    if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1 or info.st_size != lock.get('size'):
        raise ValueError('archive type/link count/size mismatch')
    if not 0 < info.st_size <= 1024 ** 3:
        raise ValueError('archive size out of bounds')
    with archive.open('rb') as stream:
        digest = hashlib.file_digest(stream, 'sha256').hexdigest()
    if digest != lock.get('sha256'):
        raise ValueError('accepted archive hash mismatch')
    if lock['product'] == 'shell':
        path = Path(directory) / 'native-evidence.json'
        if not path.is_file() or path.is_symlink() or path.stat().st_size > 4 * 1024 ** 2:
            raise ValueError('missing/invalid native evidence')
        evidence = json.loads(path.read_text())
        if evidence.get('sourceCommit') != tag_commit or evidence.get('archive', {}).get('archiveSha256') != digest or evidence.get('archive', {}).get('archiveSize') != info.st_size:
            raise ValueError('native evidence differs from accepted source/archive')
    return archive


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--lock', type=Path, required=True)
    parser.add_argument('--directory', type=Path, required=True)
    parser.add_argument('--tag-commit', required=True)
    args = parser.parse_args()
    archive = validate(json.loads(args.lock.read_text()), args.directory, args.tag_commit)
    print(json.dumps(dict(archive=archive.name, accepted=True)))
