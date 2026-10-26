#!/usr/bin/env python3
"""Sign exact reviewed two-architecture drafts; never publish or replace assets."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]

def run(*args):
    return subprocess.check_output(list(map(str, args)), text=True)

def api(path):
    return json.loads(run('gh', 'api', path))

def select_release(pages, tag):
    matches = [r for page in pages for r in page if r['tag_name'] == tag]
    if len(matches) != 1 or matches[0]['draft'] is not True:
        raise ValueError('expected exactly one unpublished draft')
    return matches[0]

def validate(lock, directory, commit):
    if not re.fullmatch(r'[a-f0-9]{40}', lock['sourceCommit']) or commit != lock['sourceCommit']:
        raise ValueError('tag must identify reviewed build source')
    expected = {'shell': ('stable', '0.1.2', 1, 'v0.1.2'), 'runtime': ('next', '0.2.0-rc.2.ocean.2', 2, 'runtime-next-v0.2.0-rc.2.ocean.2-r2')}
    if lock['product'] not in expected or tuple(lock[k] for k in ('channel','release','sequence','tag')) != expected[lock['product']]:
        raise ValueError('unreviewed release identity')
    if sorted(x['target'] for x in lock['artifacts']) != ['linux-aarch64','linux-x86_64']:
        raise ValueError('both distinct architectures required')
    for item in lock['artifacts']:
        name=item['filename']
        if not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9._-]*\.tar\.gz', name):
            raise ValueError('invalid archive basename')
        path=directory/name; info=path.lstat()
        if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1 or info.st_size != item['size'] or not 0 < info.st_size <= 1024**3:
            raise ValueError('archive type/link/size mismatch')
        with path.open('rb') as f: digest=hashlib.file_digest(f,'sha256').hexdigest()
        if digest != item['sha256']: raise ValueError('archive hash mismatch')

def asset(release, name):
    items=[x for x in release['assets'] if x['name']==name]
    if len(items)!=1: raise ValueError('missing/duplicate asset')
    return items[0]

def download(repo, release, name, destination):
    identity=asset(release,name)
    with destination.open('xb') as out:
        subprocess.run(['gh','api','-H','Accept: application/octet-stream',f'repos/{repo}/releases/assets/{identity["id"]}'],stdout=out,check=True)
    return identity['id']

def release(repo, lock):
    pages=json.loads(run('gh','api','--paginate','--slurp',f'repos/{repo}/releases?per_page=100'))
    return select_release(pages,lock['tag'])

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('phase',choices=['prepare','verify','upload'])
    parser.add_argument('--verifier',type=Path)
    args=parser.parse_args()
    product=os.environ['PRODUCT']; repo=os.environ['GITHUB_REPOSITORY']
    if repo != 'ryltsevmikhail/dsh-ocean': raise ValueError('repository mismatch')
    temp=Path(os.environ['RUNNER_TEMP']); inputs=temp/'prebuilt'
    lock=json.loads((ROOT/'docs/releases/production-prebuilt-lock.json').read_text())[product]
    def commit(): return api(f'repos/{repo}/commits/{lock["tag"]}')['sha']
    common=['--product',product,'--channel',lock['channel'],'--release',lock['release'],'--sequence',str(lock['sequence'])]
    script=ROOT/'scripts/release-artifacts.py'
    def policy(*flags): return json.loads(run(sys.executable,script,*flags))
    if args.phase=='prepare':
        inputs.mkdir(mode=0o700)
        initial=release(repo,lock)
        (temp/'draft.json').write_text(json.dumps(initial))
        (temp/'lock.json').write_text(json.dumps(lock))
        for item in lock['artifacts']: download(repo,initial,item['filename'],inputs/item['filename'])
        validate(lock,inputs,commit())
        policy('plan',*common,'--source-commit',lock['sourceCommit'],'--tag-commit',commit())
        flags=['manifest',*common,'--valid-hours','336']
        for item in lock['artifacts']:
            checked=policy('artifact',*common,'--target',item['target'],'--file',inputs/item['filename'])
            path=inputs/(item['target']+'.json'); path.write_text(json.dumps(checked))
            flags+=['--artifact',str(path)]
        (temp/'manifest.json').write_text(json.dumps(policy(*flags),indent=2)+'\n')
    elif args.phase=='verify':
        validate(lock,inputs,commit())
        if not args.verifier: raise ValueError('verifier required')
        for item in lock['artifacts']:
            command=[str(args.verifier),'--import-runtime-release' if product=='runtime' else '--verify-release',str(temp/'manifest.json'),str(temp/'signature/signature.json'),str(temp/'signature/public-key.json'),'--channel',lock['channel'],'--target',item['target'],'--min-sequence',str(lock['sequence']),'--artifact',str(inputs/item['filename'])]
            command+=['--current-runtime-version','0.0.0-0','--prefix',str(temp/('verified-runtime-'+item['target'])),'--confirm-import','yes'] if product=='runtime' else ['--kind','tar.gz']
            subprocess.run(command,check=True)
    else:
        initial=json.loads((temp/'draft.json').read_text()); current=release(repo,lock)
        if current['id']!=initial['id']: raise ValueError('draft replaced')
        validate(lock,inputs,commit())
        # Re-fetch and hash all exact archives before any metadata upload.
        remote=temp/'remote-final'; remote.mkdir(mode=0o700)
        for item in lock['artifacts']:
            name=item['filename']
            if asset(initial,name)['id'] != asset(current,name)['id']: raise ValueError('asset replaced')
            download(repo,current,name,remote/name)
        validate(lock,remote,commit())
        names=['manifest.json','signature.json','public-key.json']
        if any(x['name'] in names for x in current['assets']): raise ValueError('metadata already exists; investigate without overwrite')
        local=[temp/'manifest.json',temp/'signature/signature.json',temp/'signature/public-key.json']
        # No tag-based upload lookup; retain the captured draft identity.
        # The separate publisher must not run concurrently with this signer.
        for name,path in zip(names,local):
            before=api(f'repos/{repo}/releases/{initial["id"]}')
            if before['draft'] is not True or before['tag_name']!=lock['tag']:
                raise ValueError('captured release no longer an unpublished draft')
            if any(x['name']==name for x in before['assets']):
                raise ValueError('metadata already exists; no overwrite')
            subprocess.run(['gh','api','--method','POST','-H','Content-Type: application/json',f'https://uploads.github.com/repos/{repo}/releases/{initial["id"]}/assets?name={name}','--input',str(path)],check=True,stdout=subprocess.DEVNULL)
        final=release(repo,lock)
        if final['id']!=initial['id']: raise ValueError('draft replaced after upload')
        for name,path in zip(names,local):
            download(repo,final,name,remote/name)
            if (remote/name).read_bytes()!=path.read_bytes(): raise ValueError('remote signed metadata differs')
        latest=release(repo,lock)
        if latest['id']!=initial['id'] or commit()!=lock['sourceCommit']: raise ValueError('draft/tag changed')
        for item in lock['artifacts']:
            if asset(latest,item['filename'])['id']!=asset(initial,item['filename'])['id']: raise ValueError('archive changed after upload')
        print(json.dumps({'signedDraftChecked':True,'published':False,'releaseId':final['id']}))

if __name__=='__main__': main()
