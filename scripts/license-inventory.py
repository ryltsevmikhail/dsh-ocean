#!/usr/bin/env python3
"""Bounded, deterministic Cargo source-license inventory; not legal certification."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import tomllib
import tarfile
import io
import selectors
import time

TARGETS = ('aarch64-unknown-linux-gnu', 'x86_64-unknown-linux-gnu')
MPL = {'cssparser': '0.37.0', 'cssparser-macros': '0.7.1', 'dtoa-short': '0.3.5', 'option-ext': '0.2.0', 'selectors': '0.38.0'}
BASE_LICENSE_INPUTS = {'LICENSE', 'NOTICE', 'THIRD_PARTY_NOTICES.txt', 'locales/dsh/LICENSE', 'docs/third-party/inventory.json', 'docs/BINARY_LICENSE_AUDIT.md'}

# Exact retained release-toolchain profile; not arbitrary notice paths.
BASE_LICENSE_INPUTS |= {'docs/toolchain/rust-1.90.0/index.json',
    'docs/toolchain/rust-1.90.0/COPYRIGHT-library.html',
    'docs/toolchain/rust-1.90.0/COPYRIGHT.html',
    'docs/toolchain/rust-1.90.0/licenses/Apache-2.0.txt',
    'docs/toolchain/rust-1.90.0/licenses/BSD-2-Clause.txt',
    'docs/toolchain/rust-1.90.0/licenses/CC-BY-SA-4.0.txt',
    'docs/toolchain/rust-1.90.0/licenses/GCC-exception-3.1.txt',
    'docs/toolchain/rust-1.90.0/licenses/GPL-2.0-only.txt',
    'docs/toolchain/rust-1.90.0/licenses/GPL-3.0-or-later.txt',
    'docs/toolchain/rust-1.90.0/licenses/ISC.txt',
    'docs/toolchain/rust-1.90.0/licenses/LLVM-exception.txt',
    'docs/toolchain/rust-1.90.0/licenses/MIT.txt',
    'docs/toolchain/rust-1.90.0/licenses/NCSA.txt',
    'docs/toolchain/rust-1.90.0/licenses/OFL-1.1.txt',
    'docs/toolchain/rust-1.90.0/licenses/Unicode-3.0.txt',
    'docs/toolchain/rust-1.90.0/LICENSE-MIT',
    'docs/toolchain/rust-1.90.0/LICENSE-APACHE',
    'docs/toolchain/rust-1.90.0/LICENSE-COMPILER-BUILTINS',
    'docs/toolchain/rust-1.90.0/LICENSE-COMPILER-BUILTINS-LIBM',
    'docs/toolchain/rust-1.90.0/SOURCE-COMPILER-BUILTINS-Cargo.toml',
    'docs/toolchain/rust-1.90.0/SOURCE-UNWIND-Cargo.toml',
    'docs/toolchain/rust-1.90.0/SOURCE-UNWIND-lib.rs',
    'docs/toolchain/rust-1.90.0/COPYRIGHT-guide.txt',
    'docs/toolchain/rust-1.90.0/rust-src-1.90.0.tar.xz',
    'docs/toolchain/rust-1.90.0/COMPILER-RT-CREDITS.TXT',
    'docs/toolchain/rust-1.90.0/GNU-glibc-source.dsc',
    'docs/toolchain/rust-1.90.0/GNU-glibc-copyright',
    'docs/toolchain/rust-1.90.0/GNU-gcc-13-source.dsc',
    'docs/toolchain/rust-1.90.0/GNU-gcc-13-copyright',
    'docs/toolchain/rust-1.90.0/GNU-glibc-x86_64-start.S',
    'docs/toolchain/rust-1.90.0/GNU-glibc-x86_64-crti.S',
    'docs/toolchain/rust-1.90.0/GNU-glibc-x86_64-crtn.S',
    'docs/toolchain/rust-1.90.0/GNU-glibc-aarch64-start.S',
    'docs/toolchain/rust-1.90.0/GNU-glibc-aarch64-crti.S',
    'docs/toolchain/rust-1.90.0/GNU-glibc-aarch64-crtn.S',
    'docs/toolchain/rust-1.90.0/GNU-glibc-COPYING.LIB',
    'docs/toolchain/rust-1.90.0/GNU-gcc-crtstuff.c',
    'docs/toolchain/rust-1.90.0/COMPILER-RT-AARCH64-source.tar.gz',
    'docs/toolchain/rust-1.90.0/COMPILER-RT-AARCH64-index.json'}
# SHA-256 of the provenance blob in the asserted, immutable public 86bb883ae6979427a50bb7bca528abc668588e3d tree.
GLIB_PROVENANCE_SHA256 = '9768d4c48c8efab1c3f6d51fe35a9f22c7da69e40dfbf4415449cd9faed86179'
GLIB_PATCHED_SHA256 = 'a0f5ee8acb8faa089bcdfbc9a57372609fce7654026ccef7d9a224d05a654ccc'
REGISTRY = 'registry+https://github.com/rust-lang/crates.io-index'
MAX_FILE = 2_000_000
MAX_TOTAL = 16_000_000
LIMITS = ['Conservative default-feature Cargo source graph union, not an exact linked SBOM. Build/proc-macro edges and target predicates are recorded, not a precise host/target compilation simulation.', 'OR expressions, legacy slash declarations, embedded third-party terms and unknown attribution require manual review; retaining supplied terms does not select or approve a license.', 'Release gates: Rust std/compiler-builtins/unwind; native and dlopen libraries; actual AppImage runtime and contents; subsequent runtime downloads; art attribution and contributor authority. These are not completely audited here.', 'No claim that the entire executable is MIT or that release is legally approved.']

def sha(data):
    return hashlib.sha256(data).hexdigest()

def bounded(path, limit=MAX_FILE):
    path = Path(path)
    # Reject symlinks in every component, including cache roots.
    for p in (path, *path.parents):
        if p.is_symlink():
            raise ValueError(f'symlink not allowed: {path.name}')
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC)
    with os.fdopen(fd, 'rb') as f:
        if not stat.S_ISREG(os.fstat(f.fileno()).st_mode):
            raise ValueError('not a regular file')
        data = f.read(limit + 1)
    if len(data) > limit:
        raise ValueError(f'file exceeds limit: {path.name}')
    return data

def write_generated(output, content):
    output = Path(output)
    if any(p.is_symlink() for p in (output, *output.parents)):
        raise ValueError('symlink output not allowed')
    # Inspect the opened inode before truncating: a FIFO must not block and a
    # hardlink must not clobber an unrelated alias outside the generated tree.
    fd = os.open(output, os.O_WRONLY | os.O_CREAT | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC, 0o644)
    with os.fdopen(fd, 'w', encoding='utf-8', newline='\n') as stream:
        info = os.fstat(stream.fileno())
        if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1 or info.st_uid != os.geteuid():
            raise ValueError('generated output is not an owned single-link regular file')
        os.ftruncate(stream.fileno(), 0)
        stream.write(content)


def license_files(base):
    count = 0
    for directory, dirs, files in os.walk(base, followlinks=False):
        count += 1 + len(dirs)
        if count > 30000 or len(Path(directory).relative_to(base).parts) > 64:
            raise ValueError('package directory traversal limit')
        dirs.sort()
        for name in dirs + files:
            if (Path(directory) / name).is_symlink():
                raise ValueError(f'symlink in package: {name}')
        for name in sorted(files):
            count += 1
            if count > 30000:
                raise ValueError('package traversal limit')
            if re.match(r'^(licen[cs]e|copying|copyright|notice)(?:[._-]|$)', name, re.I):
                yield Path(directory) / name

def validate_license_inputs(root):
    paths = bounded(root / 'packaging/license-inputs.txt', 65536).decode('utf-8').splitlines()
    expected = BASE_LICENSE_INPUTS | {f'docs/third-party/sources/{name}-{version}.crate' for name, version in MPL.items()}
    if len(paths) != len(expected) or set(paths) != expected:
        raise ValueError('license payload manifest differs from exact reviewed path set')


def validate_glib_source(root):
    provenance_bytes = bounded(root / 'vendor/glib-0.18.5.provenance.json')
    if sha(provenance_bytes) != GLIB_PROVENANCE_SHA256:
        raise ValueError('glib provenance differs from trusted public tree')
    backport = json.loads(provenance_bytes)
    if (backport['version'] != '0.18.5' or backport['patched_file'] != 'src/variant_iter.rs'
            or backport['patched_sha256'] != GLIB_PATCHED_SHA256):
        raise ValueError('unreviewed glib backport version/source')
    expected = dict(backport['original_files'])
    if len(expected) != 121:
        raise ValueError('incomplete trusted glib source inventory')
    expected[backport['patched_file']] = GLIB_PATCHED_SHA256
    expected_dirs = {str(parent) for name in expected for parent in Path(name).parents if str(parent) != '.'}
    base = root / 'vendor/glib'
    observed = set()
    directories = 0
    for directory, dirs, files in os.walk(base, followlinks=False):
        directories += 1
        if directories > 30000 or len(Path(directory).relative_to(base).parts) > 64:
            raise ValueError('glib source traversal limit')
        for name in dirs:
            path = Path(directory) / name
            if path.is_symlink():
                raise ValueError('glib source symlink')
            if path.relative_to(base).as_posix() not in expected_dirs:
                raise ValueError('glib source extra/missing files')
        for name in files:
            path = Path(directory) / name
            relative = path.relative_to(base).as_posix()
            if relative not in expected:
                raise ValueError('glib source extra/missing files')
            info = path.lstat()
            if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1:
                raise ValueError('glib source must be regular, without links')
            if sha(bounded(path)) != expected[relative]:
                raise ValueError('glib source changed: ' + relative)
            observed.add(relative)
    if observed != set(expected):
        raise ValueError('glib source extra/missing files')
    return backport, sha(provenance_bytes)


def generate(root, metadata):
    validate_license_inputs(root)
    backport, backport_provenance_sha256 = validate_glib_source(root)
    lock_bytes = bounded(root / 'Cargo.lock')
    lock = tomllib.loads(lock_bytes.decode())
    locks = {(p['name'], p['version'], p.get('source')): p for p in lock['package']}
    evidence_dir = root / 'docs/third-party/evidence'
    provenance_bytes = bounded(evidence_dir / 'index.json')
    if sha(provenance_bytes) != '54d50911302a950d0cfab9534123aedb8bff7fcd88ef7e329ee3313c1b93a756':
        raise ValueError('unreviewed supplier provenance index')
    provenance = json.loads(provenance_bytes)
    expected = {'index.json'} | {x['file'] for x in provenance}
    if set(os.listdir(evidence_dir)) != expected:
        raise ValueError('unexpected or missing evidence file')
    for e in provenance:
        if Path(e['file']).name != e['file'] or sha(bounded(evidence_dir / e['file'])) != e['sha256']:
            raise ValueError('evidence checksum/path mismatch')
    packages, nodes, root_features = {}, {}, {}
    for target in TARGETS:
        m = metadata[target]
        root_id = m['resolve']['root']
        rp = next(p for p in m['packages'] if p['id'] == root_id)
        if Path(rp['manifest_path']).resolve() != (root / 'Cargo.toml').resolve():
            raise ValueError('metadata root mismatch')
        if len(m['packages']) > 5000 or len(m['resolve']['nodes']) > 5000:
            raise ValueError('metadata package limit')
        for n in m['resolve']['nodes']:
            nodes[(target, n['id'])] = n
        root_features[target] = sorted(nodes[(target, root_id)]['features'])
        # Resolve nodes are the platform-filtered graph; packages may contain extras.
        active = {n['id'] for n in m['resolve']['nodes']}
        for p in m['packages']:
            if p['id'] != root_id and p['id'] in active:
                packages.setdefault(p['id'], (p, []))[1].append(target)
    # Propagate host-tool contexts; Cargo does not expose actual linked membership.
    contexts = {}
    for target in TARGETS:
        m = metadata[target]
        proc_ids = {p['id'] for p in m['packages'] if any('proc-macro' in t['kind'] for t in p['targets'])}
        todo = [(m['resolve']['root'], 'normal')]
        visited = set()
        while todo:
            pid, context = todo.pop()
            if (pid, context) in visited:
                continue
            visited.add((pid, context))
            contexts.setdefault(pid, set()).add(context)
            for dep in nodes[(target, pid)]['deps']:
                for kind in dep['dep_kinds']:
                    child_context = 'proc-macro' if dep['pkg'] in proc_ids else ('build' if kind['kind'] == 'build' else context)
                    todo.append((dep['pkg'], child_context))
    texts, records, total = {}, [], 0
    for pid, (p, targets) in sorted(packages.items(), key=lambda item: (item[1][0]['name'], item[1][0]['version'])):
        name, version, source = p['name'], p['version'], p['source']
        base = Path(p['manifest_path']).parent
        if (name, version, source) not in locks:
            raise ValueError(f'stale metadata/lock: {name}')
        checksum = locks[(name, version, source)].get('checksum')
        if source == REGISTRY:
            if base.name != f'{name}-{version}' or base.parent.parent.name != 'src':
                raise ValueError('unsupported registry source path')
            archive_path = base.parent.parent.parent / 'cache' / base.parent.name / (base.name + '.crate')
            archive_data = bounded(archive_path, 16_000_000)
            if sha(archive_data) != checksum:
                raise ValueError('registry archive checksum differs from lock')
            checks = {'files': {}}
            covered_files = {}
            with tarfile.open(fileobj=io.BytesIO(archive_data), mode='r:gz') as archive:
                expanded = 0
                for i, member in enumerate(archive):
                    expanded += member.size
                    if i > 30000 or expanded > 128_000_000:
                        raise ValueError('archive traversal limit')
                    parts = Path(member.name).parts
                    if not parts or parts[0] != base.name or '..' in parts or member.name.startswith('/') or member.issym() or member.islnk():
                        raise ValueError('unsupported archive member path/type')
                    if name in MPL and member.isfile():
                        rel = '/'.join(parts[1:])
                        if member.size > MAX_FILE or rel in covered_files:
                            raise ValueError('MPL source member limit/duplicate')
                        covered_files[rel] = sha(archive.extractfile(member).read(MAX_FILE + 1))
                    if member.isfile() and re.match(r'^(licen[cs]e|copying|copyright|notice)(?:[._-]|$)', parts[-1], re.I):
                        if member.size > MAX_FILE:
                            raise ValueError('archive license size limit')
                        checks['files']['/'.join(parts[1:])] = sha(archive.extractfile(member).read(MAX_FILE + 1))
            if name in MPL:
                for rel, digest in covered_files.items():
                    if sha(bounded(base / rel)) != digest:
                        raise ValueError('MPL cached source modified; published archive insufficient')
                observed = set()
                for directory, dirs, files in os.walk(base, followlinks=False):
                    for entry in dirs + files:
                        if (Path(directory) / entry).is_symlink():
                            raise ValueError('MPL cached source symlink')
                    observed.update((Path(directory) / entry).relative_to(base).as_posix() for entry in files)
                    if len(observed) > 30000:
                        raise ValueError('MPL source traversal limit')
                if observed - {'.cargo-ok', '.cargo-checksum.json'} != set(covered_files):
                    raise ValueError('MPL cached source extra/missing files')
            # Check cached evidence against the lock-verified published archive.
            source_url = f'https://crates.io/api/v1/crates/{name}/{version}/download'
        elif source is None and name == 'glib' and base.resolve() == (root / 'vendor/glib').resolve():
            checks = None
            source_url = 'https://github.com/ryltsevmikhail/dsh-ocean/tree/86bb883ae6979427a50bb7bca528abc668588e3d/vendor/glib'
        else:
            raise ValueError(f'unsupported source: {name}')
        refs = []
        def add(data, path, origin):
            nonlocal total
            data.decode('utf-8')
            total += len(data)
            if total > MAX_TOTAL:
                raise ValueError('total evidence limit')
            digest = sha(data)
            texts[digest] = data.decode('utf-8')
            refs.append({'path': path, 'sha256': digest, 'origin': origin})
        for file in license_files(base):
            rel = file.relative_to(base).as_posix()
            data = bounded(file)
            if checks is not None and checks['files'].get(rel) != sha(data):
                raise ValueError(f'license content differs from published checksum: {name}/{rel}')
            add(data, rel, 'published crate' if source else 'vendored source')
        if checks is not None and {r['path'] for r in refs} != set(checks['files']):
            raise ValueError(f'missing published license file: {name}')
        for e in provenance:
            if f'{name}@{version}' in e['packages']:
                add(bounded(evidence_dir / e['file']), 'docs/third-party/evidence/' + e['file'], e['url'])
        if not refs:
            raise ValueError(f'missing license evidence: {name}@{version}')
        modes, edges = set(), []
        for target in targets:
            for (t, _), node in nodes.items():
                if t != target:
                    continue
                for dep in node['deps']:
                    if dep['pkg'] == pid:
                        for kind in dep['dep_kinds']:
                            mode = kind['kind'] or 'normal'
                            modes.add(mode)
                            edges.append({'platform': target, 'kind': mode, 'target_predicate': kind['target']})
        proc = any('proc-macro' in t['kind'] for t in p['targets'])
        r = {'name': name, 'version': version, 'license': p['license'], 'license_file': p['license_file'], 'source': source, 'source_url': source_url, 'archive_sha256': checksum, 'manifest_sha256': sha(bounded(base / 'Cargo.toml')), 'platforms': sorted(targets), 'features': {t: sorted(nodes[(t, pid)]['features']) for t in sorted(targets)}, 'dependency_modes': sorted(modes), 'conservative_graph_contexts': sorted(contexts.get(pid, [])), 'proc_macro': proc, 'incoming_edge_kinds': [json.loads(e) for e in sorted({json.dumps(e, sort_keys=True) for e in edges})], 'targets': [{'name': t['name'], 'kind': t['kind'], 'crate_types': t['crate_types']} for t in p['targets']], 'evidence': refs, 'modifications': 'No local modification asserted; license files checked against published extraction checksums.' if source else 'Local gtk-rs glib 0.18 backport for RUSTSEC-2024-0429; fixed public source tree 86bb883ae6979427a50bb7bca528abc668588e3d; not an upstream version bump.', 'manual_review': not p['license'] or 'OR' in (p['license'] or '') or '/' in (p['license'] or '')}
        if source is None:
            if version != '0.18.5':
                raise ValueError('unreviewed glib backport version/source')
            r['backport'] = {k: backport[k] for k in ('archive_url', 'archive_sha256', 'upstream_source_commit', 'upstream_fix_pr', 'upstream_fix_commit', 'advisories', 'patched_file', 'patched_sha256')}
            r['backport']['provenance_path'] = 'vendor/glib-0.18.5.provenance.json'
            r['backport']['provenance_sha256'] = backport_provenance_sha256
        if name in MPL:
            if version != MPL[name]:
                raise ValueError('MPL version requires reviewed update')
            archive = f'docs/third-party/sources/{name}-{version}.crate'
            if sha(bounded(root / archive)) != checksum:
                raise ValueError('MPL source archive checksum mismatch')
            r['recipient_source'] = archive
            r['modifications'] = 'Complete cached covered source matched the lock-verified published archive; no modifications detected.'
        records.append(r)
    expected_sources = {f'{n}-{v}.crate' for n, v in MPL.items()}
    if set(os.listdir(root / 'docs/third-party/sources')) != expected_sources:
        raise ValueError('unexpected/missing source archive')
    inventory = {'schema_version': 1, 'scope': LIMITS, 'platforms': list(TARGETS), 'root_default_features': True, 'root_features': root_features, 'metadata_command': 'cargo metadata --locked --format-version 1 --filter-platform TARGET (from root, no feature overrides)', 'cargo_lock_sha256': sha(lock_bytes), 'root_manifest_sha256': sha(bounded(root / 'Cargo.toml')), 'supplier_evidence': provenance, 'packages': records, 'text_sha256': sorted(texts)}
    notices = 'THIRD-PARTY SOURCE LICENSE INVENTORY\n\n' + '\n\n'.join(LIMITS) + '\n\nMPL covered source is bundled in docs/third-party/sources/*.crate. Extract each with:\n  tar -xzf <name>-<version>.crate\nThese are exact published source archives verified against Cargo.lock; retain license headers.\nVendored glib backport source: https://github.com/ryltsevmikhail/dsh-ocean/tree/86bb883ae6979427a50bb7bca528abc668588e3d/vendor/glib\n\nPACKAGE INDEX\n'
    for r in records:
        notices += f"{r['name']} {r['version']} | declared: {r['license']} | modes: {','.join(r['dependency_modes'])} | proc-macro: {r['proc_macro']}\n"
        if 'recipient_source' in r:
            notices += '  Bundled covered source: ' + r['recipient_source'] + '\n'
        for e in r['evidence']:
            notices += f"  {e['path']} | SHA256 {e['sha256']}\n"
    for digest, text in sorted(texts.items()):
        notices += '\n' + '=' * 72 + '\nSHA256 ' + digest + '\n' + text + '\n'
    return {'docs/third-party/inventory.json': json.dumps(inventory, indent=2, sort_keys=True) + '\n', 'THIRD_PARTY_NOTICES.txt': notices}

def cargo_metadata(root, target):
    command = ['cargo', 'metadata', '--locked', '--format-version', '1', '--filter-platform', target]
    with subprocess.Popen(command, cwd=root, stdout=subprocess.PIPE) as process:
        data = bytearray()
        deadline = time.monotonic() + 180
        try:
            with selectors.DefaultSelector() as selector:
                selector.register(process.stdout, selectors.EVENT_READ)
                while True:
                    remaining = deadline - time.monotonic()
                    if remaining <= 0 or not selector.select(remaining):
                        raise ValueError('cargo metadata timeout')
                    chunk = os.read(process.stdout.fileno(), 65536)
                    if not chunk:
                        break
                    data.extend(chunk)
                    if len(data) > 32_000_000:
                        raise ValueError('metadata limit')
            if process.wait(timeout=max(0.01, deadline - time.monotonic())):
                raise ValueError('cargo metadata failed')
            return json.loads(data)
        except BaseException:
            process.kill()
            process.wait()
            raise


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--metadata', action='append', default=[], metavar='TARGET=JSON')
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    metadata = {}
    try:
        for spec in args.metadata:
            target, path = spec.split('=', 1)
            if target not in TARGETS or target in metadata:
                raise ValueError('unsupported/duplicate metadata target')
            metadata[target] = json.loads(bounded(path, 32_000_000))
        if metadata and set(metadata) != set(TARGETS):
            raise ValueError('both Linux metadata inputs are required')
        if not metadata:
            for target in TARGETS:
                metadata[target] = cargo_metadata(root, target)
        outputs = generate(root, metadata)
        for path, content in outputs.items():
            if args.check:
                if bounded(root / path, 8_000_000) != content.encode():
                    raise ValueError(f'stale output: {path}')
            else:
                write_generated(root / path, content)
        print(('Checked' if args.check else 'Generated') + ' inventory.json and THIRD_PARTY_NOTICES.txt')
    except (ValueError, OSError, KeyError, subprocess.SubprocessError) as exc:
        print(f'license inventory failed: {exc}', file=sys.stderr)
        return 1
    return 0

if __name__ == '__main__':
    sys.exit(main())
