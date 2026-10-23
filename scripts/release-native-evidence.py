#!/usr/bin/env python3
"""Artifact-bound observations for trusted fresh builds, never legal clearance.

Uses ldd only on the explicitly trusted, locally built native ELF. Do not run this
collector on downloaded/untrusted programs. No profiles/services/keys are read.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import re
import subprocess
import tarfile

ROOT=Path(__file__).resolve().parents[1]


def load(name):
    spec=importlib.util.spec_from_file_location(name,ROOT/'scripts'/name)
    module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
    return module


def digest(path):
    if not path.is_file() or path.stat().st_size>512*1024*1024:
        raise ValueError('unexpected bounded evidence file')
    h=hashlib.sha256()
    with path.open('rb') as f:
        while chunk:=f.read(1024*1024):h.update(chunk)
    return h.hexdigest()


def verify_archive(archive,binary,inputs):
    expected={'dsh-ocean'}|set(inputs)
    actual={};roots=set();total=0
    with tarfile.open(archive,'r:gz') as tar:
        for member in tar:
            parts=Path(member.name).parts
            if not parts or Path(member.name).is_absolute() or '..' in parts:
                raise ValueError('unsafe archive path')
            roots.add(parts[0])
            if member.isdir():continue
            if not member.isfile() or len(parts)<2 or member.size>512*1024*1024:
                raise ValueError('unexpected archive member')
            name='/'.join(parts[1:])
            if name not in expected or name in actual:
                raise ValueError('unexpected/duplicate archive payload')
            total+=member.size
            if total>512*1024*1024:raise ValueError('archive payload exceeds bound')
            h=hashlib.sha256()
            with tar.extractfile(member) as f:
                while chunk:=f.read(1024*1024):h.update(chunk)
            actual[name]=h.hexdigest()
    if len(roots)!=1 or set(actual)!=expected:raise ValueError('archive payload missing')
    if actual['dsh-ocean']!=digest(binary):raise ValueError('archive ELF differs from reported build')
    for name,path in inputs.items():
        if actual[name]!=digest(path):raise ValueError('archive payload differs: '+name)
    return {'archiveSha256':digest(archive),'archiveSize':archive.stat().st_size,
            'binarySha256':actual['dsh-ocean'],'memberSha256':actual}


def command(args):
    result=subprocess.run(args,capture_output=True,text=True,check=True,timeout=30,
                          env={**os.environ,'LC_ALL':'C','LD_LIBRARY_PATH':''})
    if len(result.stdout)>4*1024*1024:raise ValueError('command output exceeded bound')
    return result.stdout


def package_origin(path):
    for candidate in (str(path),str(path.resolve())):
        found=subprocess.run(['dpkg-query','-S',candidate],capture_output=True,text=True,timeout=15)
        if found.returncode==0:
            # usr-merge may prepend diversion-by/from/to records. Only an exact
            # Debian binary package identity is a package-owner row.
            owners=[line.rsplit(': ',1)[0] for line in found.stdout.splitlines() if ': ' in line]
            owners=[name for name in owners if re.fullmatch(r'[a-z0-9][a-z0-9+.-]*(?::[a-z0-9][a-z0-9-]*)?',name)]
            if len(set(owners))!=1:continue
            record=command(['dpkg-query','-W','-f=${Package}\t${Version}\t${Architecture}\t${source:Package}\t${source:Version}\n',owners[0]]).strip().split('\t')
            if len(record)!=5:raise ValueError('unexpected dpkg package metadata')
            return dict(zip(('package','version','architecture','sourcePackage','sourceVersion'),record))
    return {'status':'system origin unresolved; requires review'}


def map_references(text):
    # LOAD/discarded entries are not proof of linked code. Keep the complete map
    # and an honest reference inventory; actual membership stays a review task.
    dialect='lld' if re.search(r'VMA\s+LMA\s+Size\s+Align\s+Out\s+In\s+Symbol',text) else 'gnu-ld' if 'Archive member included' in text or re.search(r'^OUTPUT\(',text,re.M) else 'unknown'
    refs={(m.group(1),m.group(2)) for m in re.finditer(r'(/[^\s()]+\.(?:rlib|a|o))\b(?:\(([^)]+)\))?',text)}
    return {'format':dialect,'provesLinkedMembership':False,
            'references':[{'path':p,'member':m} for p,m in sorted(refs,key=lambda x:(x[0],x[1] or ''))]}


def verify_map_output(text,binary):
    outputs=re.findall(r'^OUTPUT\((\S+)',text,re.M)
    if len(outputs)!=1:raise ValueError('map output does not identify one final GNU link')
    output=Path(outputs[0])
    if not output.is_absolute() or output.parent.resolve()!=binary.resolve().parent/'deps' or not output.name.startswith('dsh_ocean-') or not output.is_file() or digest(output)!=digest(binary):
        raise ValueError('map output differs from this Cargo ELF')


def collect(args):
    if command(['git','rev-parse','HEAD']).strip()!=args.source_commit:
        raise ValueError('source commit mismatch')
    machine={'x86_64-unknown-linux-gnu':'x86_64','aarch64-unknown-linux-gnu':'aarch64'}[args.target]
    if platform.machine()!=machine:raise ValueError('native target mismatch')
    sysroot=Path(command(['rustc','--print','sysroot']).strip()).resolve()
    profile=load('check-release-toolchain.py').check(ROOT/'docs/toolchain/rust-1.90.0',sysroot,'rustc',args.target)
    pkg=load('package-tar.py')
    binding=verify_archive(args.archive,args.binary,{name:ROOT/path for name,path in pkg.INPUTS.items()})
    observed=load('artifact-license-evidence.py').inspect_elf(args.binary)
    if observed['sha256']!=binding['binarySha256']:raise ValueError('ELF changed during evidence collection')
    if args.link_map.is_symlink() or args.link_map.stat().st_size>64*1024*1024:
        raise ValueError('unexpected linker map')
    link_map=args.link_map.read_text()
    inventory=map_references(link_map)
    if inventory['format']!='gnu-ld':raise ValueError('release pipeline must use its explicit GNU BFD linker')
    verify_map_output(link_map,args.binary)
    def public_path(value):
        return str(value).replace(str(ROOT),'$WORKSPACE').replace(str(sysroot),'$SYSROOT').replace(str(args.binary.resolve().parent.parent),'$TARGET_DIR').replace(str(Path.home()),'$BUILD_HOME')
    grouped={}
    for reference in inventory['references']:
        grouped.setdefault(reference['path'],set()).add(reference['member'])
    if len(grouped)>1024:raise ValueError('too many linker references')
    static=[]
    allowed=(sysroot,args.binary.resolve().parent.parent,ROOT,Path('/usr/lib'),Path('/lib'))
    for value,members in sorted(grouped.items()):
        file=Path(value)
        entry={'file':file.name,'path':public_path(file),
               'membersReferenced':sorted(m for m in members if m is not None),
               'provesLinkedMembership':False}
        if not file.is_file():
            entry['status']='referenced input unavailable; retained for manual review'
        else:
            resolved=file.resolve()
            if not any(resolved.is_relative_to(p.resolve()) for p in allowed):
                raise ValueError('unreviewed linker reference root')
            entry.update(sha256=digest(file),size=file.stat().st_size,
                         kind='system' if str(resolved).startswith('/usr/lib/') else 'rust-build-input')
            if entry['kind']=='system':entry['origin']=package_origin(file)
        static.append(entry)
    if not any('compiler_builtins' in row['file'] for row in static):
        raise ValueError('compiler-builtins linker reference was not captured')
    dynamic=[]
    loader=command(['ldd',str(args.binary.resolve())])
    if 'not found' in loader:raise ValueError('native loader dependency missing')
    for value in sorted(set(re.findall(r'(/[^\s]+)\s+\(0x',loader))):
        file=Path(value);resolved=file.resolve()
        if not any(resolved.is_relative_to(p) for p in (Path('/usr/lib'),Path('/lib'))):
            raise ValueError('non-system loader dependency')
        dynamic.append({'soname':file.name,'sha256':digest(file),'origin':package_origin(file),
                        'redistributedByPortableTar':False})
    version_info=command(['readelf','--version-info',str(args.binary)])
    requirements=sorted(set(re.findall(r'\b(?:GLIBC(?:XX)?|CXXABI)_[0-9.]+',version_info)))
    sanitized=link_map.replace(str(ROOT),'$WORKSPACE').replace(str(sysroot),'$SYSROOT')
    sanitized=sanitized.replace(str(args.binary.resolve().parent.parent),'$TARGET_DIR')
    sanitized=sanitized.replace(str(Path.home()),'$BUILD_HOME')
    os_release={}
    for line in Path('/etc/os-release').read_text().splitlines():
        if '=' in line:
            k,v=line.split('=',1)
            if k in ('ID','VERSION_ID','PRETTY_NAME'):os_release[k]=v.strip('"')
    report={'schema':1,'sourceCommit':args.source_commit,'target':args.target,'system':os_release,
            'toolchain':profile,'archive':binding,'elf':observed,'symbolVersions':requirements,
            'linkMapFormat':inventory['format'],'linkMapReferencedInputs':static,'systemLoaderClosure':dynamic,
            'linker':{'driver':'/usr/bin/gcc','selection':'-fuse-ld=bfd',
                      'driverVersion':command(['/usr/bin/gcc','--version']).splitlines()[0],
                      'linkerVersion':command(['/usr/bin/ld.bfd','--version']).splitlines()[0]},
            'linkMapRawSha256':hashlib.sha256(args.link_map.read_bytes()).hexdigest(),
            'linkMapPublishedSha256':hashlib.sha256(sanitized.encode()).hexdigest(),
            'flags':'fresh release target; explicit /usr/bin/gcc, -fuse-ld=bfd and GNU linker map',
            'limits':['Observed on native Ubuntu24.04; not universal distro/Wayland/GPU compatibility.',
                      'Loader closure is ldd for the trusted exact ELF, not all later dlopen/plugins/subprocesses.',
                      'Map inventory is references only (including LOAD/discarded entries), NOT linked membership; missing refs are retained for review.',
                      'GNU OUTPUT must identify this exact final Cargo ELF; map section membership/notice applicability still require manual review.',
                      'Conservative compiler-wide notices do not mean all those components are linked.']}
    args.output_dir.mkdir(mode=0o700)
    (args.output_dir/'report.json').write_text(json.dumps(report,indent=2)+'\n')
    (args.output_dir/'link-map.txt').write_text(sanitized)
    print(json.dumps({'sourceCommit':args.source_commit,'target':args.target,'archiveSha256':binding['archiveSha256'],
                      'binarySha256':binding['binarySha256'],'mapReferencedInputs':len(static),'systemDependencies':len(dynamic)}))


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--archive',type=Path,required=True)
    parser.add_argument('--binary',type=Path,required=True)
    parser.add_argument('--link-map',type=Path,required=True)
    parser.add_argument('--target',choices=('x86_64-unknown-linux-gnu','aarch64-unknown-linux-gnu'),required=True)
    parser.add_argument('--source-commit',required=True)
    parser.add_argument('--output-dir',type=Path,required=True)
    args=parser.parse_args()
    try:collect(args)
    except (ValueError,OSError,subprocess.SubprocessError) as error:parser.exit(1,'native release evidence refused: '+str(error)+'\n')
