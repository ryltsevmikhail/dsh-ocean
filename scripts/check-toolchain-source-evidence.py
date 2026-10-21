#!/usr/bin/env python3
"""Verify a reviewed upstream source profile, not an ELF license clearance."""
import hashlib
import json
import os
from pathlib import Path
import stat
import sys

COMMIT = '58cf8f9c35b355199727627d07ae34a4e2846d67'
PROFILE = 'docs/toolchain/source-evidence/rust-' + COMMIT[:12]
INDEX_SHA256 = '54aeab0907f237ed80f198d6add4dcaa26c78d87fb284fbe8af49c405cec3c1b'
FILES = {'LICENSE-RUST-MIT', 'LICENSE-RUST-APACHE', 'COPYRIGHT-RUST-GUIDE',
         'LICENSE-COMPILER-BUILTINS', 'LICENSE-COMPILER-BUILTINS-LIBM',
         'compiler-builtins.Cargo.toml', 'unwind.Cargo.toml', 'unwind.lib.rs'}


def read_regular(path, limit=200000):
    path = Path(path)
    if any(part.is_symlink() for part in (path, *path.parents)):
        raise ValueError('symlink evidence')
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC)
    with os.fdopen(fd, 'rb') as stream:
        before = os.fstat(stream.fileno())
        if not stat.S_ISREG(before.st_mode) or before.st_nlink != 1 or before.st_size > limit:
            raise ValueError('evidence must be bounded regular single-link file')
        data = stream.read(limit + 1)
        after = os.fstat(stream.fileno())
        fields = ('st_dev', 'st_ino', 'st_size', 'st_mtime_ns', 'st_ctime_ns', 'st_mode', 'st_nlink', 'st_uid')
        if len(data) != before.st_size or len(data) > limit or any(getattr(before, name) != getattr(after, name) for name in fields):
            raise ValueError('evidence changed')
        return data


def validate(root):
    base = Path(root) / PROFILE
    raw = read_regular(base / 'index.json')
    if hashlib.sha256(raw).hexdigest() != INDEX_SHA256:
        raise ValueError('unreviewed source evidence index')
    index = json.loads(raw)
    if index['compiler_source_commit'] != COMMIT or set(os.listdir(base)) != FILES | {'index.json'}:
        raise ValueError('source profile file set changed')
    if len(index['files']) != len(FILES) or {row['file'] for row in index['files']} != FILES:
        raise ValueError('source profile index file set changed')
    for row in index['files']:
        data = read_regular(base / row['file'])
        if hashlib.sha256(data).hexdigest() != row['sha256']:
            raise ValueError('source evidence hash mismatch: ' + row['file'])
        blob = b'blob ' + str(len(data)).encode() + b'\0' + data
        if hashlib.sha1(blob).hexdigest() != row['git_blob_sha1']:
            raise ValueError('source evidence Git blob mismatch')
    return index


def main():
    try:
        validate(Path(__file__).resolve().parents[1])
    except (OSError, ValueError, KeyError, TypeError) as error:
        print('toolchain source evidence failed: ' + str(error), file=sys.stderr)
        return 1
    print('Pinned Rust source profile verified; compiled membership and release clearance remain unresolved.')
    return 0


if __name__ == '__main__':
    sys.exit(main())
