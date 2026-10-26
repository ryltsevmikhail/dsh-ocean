#!/usr/bin/env python3
"""Shared fail-closed GitHub ref mutex for production staging/signing/publication.

Uses a unique owner commit and atomic ref creation, never force-update. No keys.
Interrupted owners retain the ref and require explicit captured-owner recovery.
"""
import argparse
from contextlib import contextmanager
import json
import os
from pathlib import Path
import subprocess
import uuid

REPO='ryltsevmikhail/dsh-ocean'
REF='heads/codex/production-operation-lock'

def api(path,method='GET',data=None,absent=False):
    gh=os.environ.get('OCEAN_GH_BIN','gh')
    command=[gh,'api',path]
    if method!='GET':command+=['--method',method]
    if data is not None:command+=['--input','-']
    p=subprocess.run(command,input=json.dumps(data) if data is not None else None,capture_output=True,text=True,timeout=120)
    if p.returncode:
        if absent and 'HTTP 404' in p.stderr:return None
        raise RuntimeError('GitHub release lock operation failed; retain owner evidence and investigate')
    return json.loads(p.stdout) if p.stdout.strip() else None

def acquire(owner):
    if api(f'repos/{REPO}/git/ref/{REF}',absent=True) is not None:
        raise RuntimeError('another production operation owns the release lock')
    base=api(f'repos/{REPO}/git/ref/heads/main')['object']['sha']
    tree=api(f'repos/{REPO}/git/commits/{base}')['tree']['sha']
    commit=api(f'repos/{REPO}/git/commits','POST',dict(message='Production operation lock '+owner+' '+uuid.uuid4().hex,tree=tree,parents=[base]))['sha']
    # Concurrent creators race here; GitHub permits exactly one new ref.
    api(f'repos/{REPO}/git/refs','POST',dict(ref='refs/'+REF,sha=commit))
    return dict(ref=REF,owner=owner,commit=commit)

def release(token):
    current=api(f'repos/{REPO}/git/ref/{REF}',absent=True)
    if current is None:raise RuntimeError('owned release lock disappeared; investigate')
    if current['object']['sha']!=token['commit']:raise RuntimeError('release lock owner changed; refuse deletion')
    api(f'repos/{REPO}/git/refs/{REF}','DELETE')

@contextmanager
def hold(owner, tokenfile=None):
    token=acquire(owner)
    if tokenfile is not None:
        tokenfile.write_text(json.dumps(dict(token,released=False)));tokenfile.chmod(0o600)
    try:yield token
    finally:
        release(token)
        if tokenfile is not None:tokenfile.write_text(json.dumps(dict(token,released=True)))

def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('phase',choices=['acquire','release']);a=p.parse_args()
    tokenfile=Path(os.environ['RUNNER_TEMP'])/'production-operation-lock.json'
    if a.phase=='acquire':
        if tokenfile.exists():raise RuntimeError('owner token already exists')
        token=acquire('actions-'+os.environ['GITHUB_RUN_ID'])
        # Persist owner identity before any release payload mutation.
        tokenfile.write_text(json.dumps(token));tokenfile.chmod(0o600)
    elif tokenfile.exists():
        release(json.loads(tokenfile.read_text()));tokenfile.unlink()

if __name__=='__main__':main()
