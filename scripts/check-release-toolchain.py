#!/usr/bin/env python3
"""Check exact release-toolchain notice bytes; never grants native/legal clearance."""
import argparse
import hashlib
import json
from pathlib import Path
import os
import stat
import subprocess

ROOT = Path(__file__).resolve().parents[1]
INDEX_SHA256 = '0079400d088aadbb00d4b0728cf6ba7c5f5cdde24c02b41e8a951fbb1c731c9d'
COMMIT = '1159e78c4747b02ef996e55082b704c09b970588'
TARGETS = {'x86_64-unknown-linux-gnu', 'aarch64-unknown-linux-gnu'}


def read_notice(path, limit=16*1024*1024):
    if any(p.is_symlink() for p in (path, *path.parents)):
        raise ValueError('symlink notice refused')
    with path.open('rb') as f:
        before = os.fstat(f.fileno())
        if not stat.S_ISREG(before.st_mode) or before.st_nlink != 1 or before.st_size > limit:
            raise ValueError('invalid notice input')
        data = f.read(limit+1)
        after = os.fstat(f.fileno())
    if len(data)>limit or (before.st_size,before.st_mtime_ns,before.st_ctime_ns)!=(after.st_size,after.st_mtime_ns,after.st_ctime_ns):
        raise ValueError('notice changed or exceeded bound')
    return data


def check(profile, sysroot=None, rustc='rustc', target=None):
    raw = read_notice(profile/'index.json', 65536)
    if hashlib.sha256(raw).hexdigest() != INDEX_SHA256:
        raise ValueError('unreviewed release notice index')
    index = json.loads(raw)
    if index['rustcCommit'] != COMMIT or set(index['targets']) != TARGETS:
        raise ValueError('release toolchain identity mismatch')
    expected = {'index.json'}
    for entry in index['files']:
        name = entry['path']
        if Path(name).is_absolute() or '..' in Path(name).parts or name in expected:
            raise ValueError('unsafe/duplicate notice path')
        expected.add(name)
        data = read_notice(profile/name)
        if len(data) != entry['size'] or hashlib.sha256(data).hexdigest() != entry['sha256']:
            raise ValueError('release notice mismatch: '+name)
    actual = {str(p.relative_to(profile)) for p in profile.rglob('*') if not p.is_dir()}
    if actual != expected:
        raise ValueError('unexpected release notice files')
    if sysroot is not None:
        result = subprocess.run([rustc,'-vV'],check=True,capture_output=True,text=True,timeout=15)
        version = dict(line.split(': ',1) for line in result.stdout.splitlines() if ': ' in line)
        if version.get('release')!='1.90.0' or version.get('commit-hash')!=COMMIT or version.get('host')!=target or target not in TARGETS:
            raise ValueError('actual compiler/target is not the reviewed release toolchain')
        # Compare complete installed distribution notices on each native runner,
        # not the moving host compiler or just a version string.
        for entry in index['files']:
            if 'member' in entry:
                data = read_notice(sysroot/'share/doc/rust'/entry['path'])
                if hashlib.sha256(data).hexdigest()!=entry['sha256']:
                    raise ValueError('installed toolchain notice mismatch: '+entry['path'])
    return {'rustcVersion':'1.90.0','rustcCommit':COMMIT,'noticeFiles':len(index['files']),
            'installedToolchainChecked':sysroot is not None,'target':target,
            'warning':'Exact notice retention/identity, not a complete linked SBOM or legal clearance.'}


if __name__ == '__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--profile',type=Path,default=ROOT/'docs/toolchain/rust-1.90.0')
    parser.add_argument('--sysroot',type=Path)
    parser.add_argument('--rustc',default='rustc',help='trusted installed compiler executable')
    parser.add_argument('--target',choices=sorted(TARGETS))
    args=parser.parse_args()
    try:
        print(json.dumps(check(args.profile,args.sysroot,args.rustc,args.target),indent=2))
    except (ValueError,OSError,subprocess.SubprocessError) as error:
        parser.exit(1, 'release toolchain notice check refused: '+str(error)+'\n')
