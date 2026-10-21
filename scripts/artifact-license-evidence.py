#!/usr/bin/env python3
"""Bounded artifact observations, not license clearance or build attestation."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import selectors
import stat
import struct
import subprocess
import sys
import time

MAX_FILE = 2 * 1024**3
MAX_NOTICE = 4 * 1024**2
MAX_NOTICE_COUNT = 16
MAX_TOTAL_NOTICE = 16 * 1024**2
ENV_KEYS = ('RUSTC', 'RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER', 'CARGO_BUILD_TARGET',
            'RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'RUSTUP_TOOLCHAIN', 'RUSTUP_HOME',
            'CARGO_HOME', 'RUSTC_BOOTSTRAP', 'CARGO_TARGET_DIR', 'CARGO_BUILD_RUSTFLAGS',
            'CC', 'CXX', 'AR', 'LD', 'LIBRARY_PATH', 'LD_LIBRARY_PATH')


class EvidenceError(ValueError):
    pass


def digest(data):
    return hashlib.sha256(data).hexdigest()


def identity(s):
    return (s.st_dev, s.st_ino, s.st_size, s.st_mtime_ns, s.st_ctime_ns, s.st_nlink, s.st_mode, s.st_uid)


def directory_fd(path):
    """Walk without following symlinks; reject writable untrusted ancestors."""
    path = os.path.abspath(path)
    fd = os.open('/', os.O_RDONLY | os.O_DIRECTORY)
    try:
        for part in Path(path).parts[1:]:
            nxt = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd)
            os.close(fd)
            fd = nxt
            s = os.fstat(fd)
            if s.st_uid not in (0, os.getuid()) or (s.st_mode & 0o022 and not (s.st_uid == 0 and s.st_mode & stat.S_ISVTX)):
                raise EvidenceError('unsafe directory ancestor')
        return fd
    except BaseException:
        os.close(fd)
        raise


def open_regular(path, limit, single_link=True):
    parent = directory_fd(Path(path).absolute().parent)
    try:
        fd = os.open(Path(path).name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=parent)
    finally:
        os.close(parent)
    s = os.fstat(fd)
    if not stat.S_ISREG(s.st_mode) or s.st_uid != os.getuid() or (single_link and s.st_nlink != 1) or s.st_size > limit:
        os.close(fd)
        raise EvidenceError('input must be owned, single-link regular file within size limit')
    return fd, s


def read_regular(path, limit=MAX_NOTICE):
    fd, before = open_regular(path, limit)
    try:
        data = bytearray()
        while len(data) <= limit:
            chunk = os.read(fd, min(65536, limit + 1 - len(data)))
            if not chunk:
                break
            data.extend(chunk)
        after = os.fstat(fd)
        if len(data) > limit or identity(before) != identity(after):
            raise EvidenceError('input changed or exceeds limit')
        return bytes(data)
    finally:
        os.close(fd)


def sanitized_string(raw, kind):
    sha = digest(raw)
    if kind == 'needed' and re.fullmatch(rb'[A-Za-z0-9_+.-]{1,255}', raw) and raw not in (b'.', b'..'):
        return {'name': raw.decode('ascii'), 'sha256': sha}
    if kind == 'needed':
        return {'classification': 'suspicious-redacted', 'sha256': sha}
    # Never emit arbitrary loader/search paths: even relative paths can identify users.
    if raw == b'':
        category = 'empty-current-directory'
    elif raw.startswith(b'/'):
        category = 'absolute-redacted'
    elif raw.startswith((b'$ORIGIN', b'${ORIGIN}')):
        category = 'origin-relative-redacted'
    else:
        category = 'relative-or-token-redacted'
    return {'classification': category, 'sha256': sha}


def inspect_elf(path):
    # Cargo commonly hardlinks the root executable to its deps artifact. This
    # descriptor is read-only and its identity/link-count/stamp is rechecked.
    fd, before = open_regular(path, MAX_FILE, single_link=False)
    try:
        size = before.st_size
        def take(off, count):
            if off < 0 or count < 0 or off + count > size:
                raise EvidenceError('ELF bounds invalid')
            data = os.pread(fd, count, off)
            if len(data) != count:
                raise EvidenceError('short ELF read')
            return data
        h = take(0, 64)
        if h[:7] != b'\x7fELF\x02\x01\x01' or h[7] not in (0, 3) or h[8] != 0 or any(h[9:16]):
            raise EvidenceError('unsupported ELF identification')
        typ, machine, version, _, phoff, _, _, ehsize, phsize, phnum, _, _, _ = struct.unpack('<HHIQQQIHHHHHH', h[16:])
        if typ not in (2, 3) or machine not in (62, 183) or version != 1 or ehsize != 64 or phsize != 56 or not 1 <= phnum <= 1024 or phoff < 64:
            raise EvidenceError('unsupported ELF header')
        table = take(phoff, phsize * phnum)
        loads, dynamics, interps = [], [], []
        for i in range(phnum):
            ptype, _, off, va, _, filesz, memsz, align = struct.unpack_from('<IIQQQQQQ', table, i * 56)
            if filesz:
                take(off, 0)
                if off + filesz > size:
                    raise EvidenceError('segment outside ELF')
            if filesz > memsz or va + memsz > 2**64 or (align not in (0, 1) and (align & (align - 1) or (va - off) % align)):
                raise EvidenceError('invalid segment layout')
            if ptype == 1:
                if any(va < v + m and v < va + memsz for _, v, _, m in loads):
                    raise EvidenceError('ambiguous PT_LOAD mapping')
                loads.append((off, va, filesz, memsz))
            elif ptype == 2:
                dynamics.append((off, filesz, va))
            elif ptype == 3:
                interps.append((off, filesz))
        if not loads or len(dynamics) > 1 or len(interps) > 1:
            raise EvidenceError('missing loads or duplicate ELF segments')
        interpreter = None
        if interps:
            off, n = interps[0]
            if not 2 <= n <= 4096:
                raise EvidenceError('invalid interpreter size')
            raw = take(off, n)
            if raw[-1:] != b'\0' or b'\0' in raw[:-1]:
                raise EvidenceError('invalid interpreter string')
            interpreter = sanitized_string(raw[:-1], 'interpreter')
        needed, search = [], []
        if dynamics:
            off, n, dynamic_va = dynamics[0]
            mappings = [o + dynamic_va - v for o, v, f, _ in loads if v <= dynamic_va and dynamic_va + n <= v + f]
            if mappings != [off]:
                raise EvidenceError('dynamic table is not uniquely load-mapped')
            if not n or n % 16 or n > 16 * 8192:
                raise EvidenceError('invalid dynamic table size')
            tags, terminated = {}, False
            for tag, val in struct.iter_unpack('<qQ', take(off, n)):
                if tag == 0:
                    terminated = True
                    continue
                if terminated:
                    raise EvidenceError('non-null entry after DT_NULL')
                if tag in (5, 10, 15, 29) and tag in tags:
                    raise EvidenceError('duplicate dynamic singleton')
                tags.setdefault(tag, []).append(val)
            if not terminated or 5 not in tags or 10 not in tags:
                raise EvidenceError('missing dynamic terminator or string table')
            va, length = tags[5][0], tags[10][0]
            if not 1 <= length <= 4 * 1024**2:
                raise EvidenceError('invalid string table size')
            matches = [o + va - v for o, v, f, _ in loads if v <= va and va + length <= v + f]
            if len(matches) != 1:
                raise EvidenceError('unmapped or ambiguous string table')
            strings = take(matches[0], length)
            if strings[0] != 0 or strings[-1] != 0:
                raise EvidenceError('unterminated string table')
            def string_at(index):
                if index >= length:
                    raise EvidenceError('string offset outside table')
                end = strings.find(b'\0', index)
                if end < 0 or end - index > 4096:
                    raise EvidenceError('unterminated or excessive dynamic string')
                return strings[index:end]
            if len(tags.get(1, [])) > 1024:
                raise EvidenceError('excessive DT_NEEDED')
            raw_needed = [string_at(i) for i in tags.get(1, [])]
            if len(set(raw_needed)) != len(raw_needed):
                raise EvidenceError('duplicate DT_NEEDED')
            needed = [sanitized_string(raw, 'needed') for raw in raw_needed]
            for tag, name in ((15, 'RPATH'), (29, 'RUNPATH')):
                for index in tags.get(tag, []):
                    search.append({'kind': name, 'entries': [sanitized_string(x, 'search') for x in string_at(index).split(b':')]})
        sha = hashlib.sha256()
        for off in range(0, size, 1024 * 1024):
            sha.update(take(off, min(1024 * 1024, size - off)))
        if identity(os.fstat(fd)) != identity(before):
            raise EvidenceError('binary changed during observation')
        return {'sha256': sha.hexdigest(), 'size': size, 'elf': {'class': 64, 'endianness': 'little', 'machine': {62: 'x86_64', 183: 'aarch64'}[machine], 'type': {2: 'ET_EXEC', 3: 'ET_DYN'}[typ], 'interpreter': interpreter, 'direct_needed': needed, 'search_paths': search}}
    finally:
        os.close(fd)


def observe_compiler(rustc='rustc', timeout=5, limit=65536):
    # Drain both pipes concurrently; never store or publish stderr or arbitrary stdout.
    proc = subprocess.Popen([os.fspath(rustc), '-vV'], stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
    output = bytearray()
    total = 0
    deadline = time.monotonic() + timeout
    sel = selectors.DefaultSelector()
    try:
        for pipe in (proc.stdout, proc.stderr):
            os.set_blocking(pipe.fileno(), False)
            sel.register(pipe, selectors.EVENT_READ)
        while sel.get_map():
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise EvidenceError('compiler observation timeout')
            for key, _ in sel.select(min(remaining, .1)):
                chunk = os.read(key.fileobj.fileno(), 8192)
                if not chunk:
                    sel.unregister(key.fileobj)
                    continue
                total += len(chunk)
                if total > limit:
                    raise EvidenceError('compiler observation exceeds size limit')
                if key.fileobj is proc.stdout:
                    output.extend(chunk)
        if proc.wait(timeout=max(.001, deadline - time.monotonic())) != 0:
            raise EvidenceError('compiler observation failed')
    finally:
        if proc.poll() is None or sel.get_map():
            import signal
            try:
                os.killpg(proc.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        proc.wait()
        sel.close()
        proc.stdout.close()
        proc.stderr.close()
    fields = {}
    patterns = {'release': r'[0-9]+\.[0-9]+\.[0-9]+(?:-(?:nightly|beta|dev)(?:\.[0-9]+)?)?', 'commit-hash': r'[0-9a-f]{40}', 'host': r'[A-Za-z0-9_]+(?:-[A-Za-z0-9_]+){2,5}', 'LLVM version': r'[0-9]+(?:\.[0-9]+){1,3}'}
    for line in output.decode('utf-8', errors='replace').splitlines():
        key, sep, value = line.partition(': ')
        if sep and key in patterns and re.fullmatch(patterns[key], value):
            if key in fields:
                raise EvidenceError('duplicate compiler field')
            fields[key] = value
    if 'release' not in fields or 'host' not in fields:
        raise EvidenceError('compiler output lacks valid version/host')
    if 'commit-hash' in fields:
        fields['rust_source_url'] = 'https://github.com/rust-lang/rust/tree/' + fields['commit-hash']
    return fields


def create_evidence(binary, output_dir, rustc='rustc', notices=(), root=None):
    root = Path(root) if root is not None else Path(__file__).resolve().parent.parent
    labels = set()
    copies = []
    total_notice_bytes = 0
    for label, source in notices:
        if len(copies) >= MAX_NOTICE_COUNT:
            raise EvidenceError('notice count exceeds limit')
        name = Path(source).name
        if any(part.startswith('.') and part not in ('.', '..') for part in Path(os.path.abspath(source)).parts):
            raise EvidenceError('hidden profile paths are not public notice sources')
        if not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9_-]{0,63}', label) or label in labels:
            raise EvidenceError('invalid or duplicate notice label')
        if not re.fullmatch(r'(?:LICENSE|COPYRIGHT|COPYING|NOTICE)(?:[._-][A-Za-z0-9._-]{1,100})?', name, re.ASCII | re.IGNORECASE):
            raise EvidenceError('notice basename must identify a public license/notice file')
        labels.add(label)
        data = read_regular(source)
        total_notice_bytes += len(data)
        if total_notice_bytes > MAX_TOTAL_NOTICE:
            raise EvidenceError('total notice bytes exceed limit')
        copies.append((label, name, data))
    report = {'schema_version': 1, 'purpose': 'release-evidence companion; not clearance, SBOM, attestation, or installed runtime ownership proof', 'binary': inspect_elf(binary), 'compiler_observed_now': observe_compiler(rustc), 'environment_present': {k: k in os.environ for k in ENV_KEYS}, 'observed_inputs': [], 'notices': [], 'limitations': ['Compiler reported now does not prove which compiler built this arbitrary ELF or a fresh/cached Cargo artifact.', 'ELF machine is not CPU compatibility, distro compatibility, or actual target ABI acceptance.', 'Input digests do not prove build membership.', 'Raw candidate copyrights do not prove full standard-library notice coverage; COPYRIGHT-library may contain only an empty external-dependency list.', 'No claim that the whole ELF is MIT licensed.'], 'unresolved_gates': ['std/compiler-builtins/unwind license and notice coverage', 'native dependency closure and dlopen', 'AppImage/runtime/distro review', 'other asset license review']}
    for name in ('Cargo.lock', 'docs/third-party/inventory.json'):
        p = root / name
        if os.path.lexists(p):
            data = read_regular(p, 16 * 1024**2)
            report['observed_inputs'].append({'basename': Path(name).name, 'sha256': digest(data), 'size': len(data)})
    parent = directory_fd(Path(output_dir).absolute().parent)
    out = None
    try:
        os.mkdir(Path(output_dir).name, 0o700, dir_fd=parent)
        out = os.open(Path(output_dir).name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=parent)
        s = os.fstat(out)
        if s.st_uid != os.getuid() or s.st_mode & 0o077:
            raise EvidenceError('unsafe output directory')
        os.mkdir('notices', 0o700, dir_fd=out)
        nd = os.open('notices', os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=out)
        def exclusive_write(folder, name, data):
            fd = os.open(name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600, dir_fd=folder)
            try:
                s = os.fstat(fd)
                if not stat.S_ISREG(s.st_mode) or s.st_uid != os.getuid() or s.st_nlink != 1:
                    raise EvidenceError('unsafe output file')
                with os.fdopen(fd, 'wb', closefd=False) as stream:
                    stream.write(data)
            finally:
                os.close(fd)
        try:
            for label, name, data in sorted(copies):
                dest = label + '-' + name
                exclusive_write(nd, dest, data)
                report['notices'].append({'label': label, 'basename': name, 'file': 'notices/' + dest, 'sha256': digest(data), 'size': len(data), 'status': 'candidate-not-coverage-proof'})
        finally:
            os.close(nd)
        exclusive_write(out, 'report.json', (json.dumps(report, sort_keys=True, indent=2) + '\n').encode())
    finally:
        if out is not None:
            os.close(out)
        os.close(parent)
    return report


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', required=True)
    parser.add_argument('--output-dir', required=True)
    parser.add_argument('--rustc', default='rustc')
    parser.add_argument('--notice', action='append', default=[], metavar='LABEL=FILE')
    args = parser.parse_args(argv)
    try:
        notices = []
        for value in args.notice:
            label, sep, path = value.partition('=')
            if not sep or not path:
                raise EvidenceError('notice requires LABEL=FILE')
            notices.append((label, path))
        create_evidence(args.binary, args.output_dir, args.rustc, notices)
    except (OSError, ValueError, subprocess.SubprocessError):
        # Avoid exception text: OS and compiler failures may contain private paths.
        print('artifact evidence failed; inputs/output must be safe and supported; any private partial output is retained', file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
