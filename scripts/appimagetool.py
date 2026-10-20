#!/usr/bin/env python3
"""Explicit operator digest pinning. No implicit publisher trust or tool execution."""
import argparse
import hashlib
import os
from pathlib import Path
import platform
import re
import stat
import sys
import tempfile
import urllib.parse
import urllib.request

MAX_TOOL = 256 * 1024 * 1024


def digest(value):
    if not re.fullmatch(r"[0-9a-fA-F]{64}", value):
        raise ValueError("an explicit SHA-256 digest is required")
    return value.lower()


def https_url(value):
    parsed = urllib.parse.urlsplit(value)
    if parsed.scheme != "https" or not parsed.hostname or parsed.username or parsed.password or parsed.fragment:
        raise ValueError("tool URL must be explicit credential-free HTTPS")
    return value


def identity(info):
    return (info.st_dev, info.st_ino, info.st_size, info.st_mtime_ns, info.st_ctime_ns, info.st_mode)


def copy_verified(source, destination, expected):
    expected = digest(expected)
    result = hashlib.sha256()
    total = 0
    fd = os.open(destination, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_CLOEXEC | os.O_NOFOLLOW, 0o600)
    owned = os.fstat(fd)
    try:
        os.fchmod(fd, 0o600)
        with os.fdopen(os.dup(fd), "wb") as target:
            while True:
                block = source.read(65536)
                if not block:
                    break
                if not isinstance(block, bytes):
                    raise ValueError("invalid tool stream")
                total += len(block)
                if total > MAX_TOOL:
                    raise ValueError("tool exceeds bounded size")
                result.update(block)
                target.write(block)
            if not total or result.hexdigest() != expected:
                raise ValueError("tool digest/size mismatch")
            target.flush()
            os.fsync(target.fileno())
        return owned
    except BaseException:
        current = destination.lstat()
        if (current.st_dev, current.st_ino) == (owned.st_dev, owned.st_ino):
            destination.unlink()
        raise
    finally:
        os.close(fd)


def snapshot_tool(path, destination, expected):
    digest(expected)
    fd = os.open(path, os.O_RDONLY | os.O_NONBLOCK | os.O_NOFOLLOW | os.O_CLOEXEC)
    with os.fdopen(fd, "rb") as source:
        before = os.fstat(source.fileno())
        if (not stat.S_ISREG(before.st_mode) or before.st_size > MAX_TOOL or before.st_nlink != 1
                or before.st_uid != os.geteuid() or before.st_mode & 0o022):
            raise ValueError("tool must be bounded, owned, singly linked and not writable by others")
        copy_verified(source, destination, expected)
        if identity(os.fstat(source.fileno())) != identity(before) or identity(path.lstat()) != identity(before):
            raise ValueError("tool changed while reading")
    # Only the verified private copy becomes executable, never a cached download.
    destination.chmod(0o700)
    return destination


class HTTPSRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, message, headers, newurl):
        # Validate BEFORE urllib opens the next hop, not after it was fetched.
        https_url(newurl)
        return super().redirect_request(request, fp, code, message, headers, newurl)


def open_https(url, timeout):
    return urllib.request.build_opener(HTTPSRedirect()).open(url, timeout=timeout)


def fetch(url, expected, directory, opener=None):
    expected = digest(expected)
    https_url(url)
    arch = platform.machine()
    if platform.system() != "Linux" or arch not in ("x86_64", "aarch64"):
        raise ValueError("unsupported native tool architecture")
    output = directory / f"appimagetool-{arch}-{expected}.AppImage"
    if os.path.lexists(output):
        raise FileExistsError("tool cache output already exists")
    directory.mkdir(mode=0o700, parents=True, exist_ok=True)
    fd = os.open(directory, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
    try:
        held = os.fstat(fd)
        if held.st_uid != os.geteuid() or held.st_mode & 0o022:
            raise ValueError("tool cache must be owned and not writable by others")
        with tempfile.TemporaryDirectory(prefix=".appimagetool-", dir=directory) as temporary:
            verified = Path(temporary) / "verified-tool"
            with (opener or open_https)(url, timeout=30) as response:
                https_url(response.geturl())
                owned = copy_verified(response, verified, expected)
            current = directory.lstat()
            if not stat.S_ISDIR(current.st_mode) or (current.st_dev, current.st_ino) != (held.st_dev, held.st_ino):
                raise ValueError("tool cache directory changed")
            os.link(verified, output.name, dst_dir_fd=fd, follow_symlinks=False)
            published = os.stat(output.name, dir_fd=fd, follow_symlinks=False)
            if (published.st_dev, published.st_ino) != (owned.st_dev, owned.st_ino):
                raise ValueError("tool cache publication changed")
        os.fsync(fd)
        return output
    finally:
        os.close(fd)


def main():
    parser = argparse.ArgumentParser(description="Fetch appimagetool only with an independently chosen SHA-256; never execute it.")
    parser.add_argument("--url", required=True)
    parser.add_argument("--sha256", required=True)
    parser.add_argument("--output-dir", type=Path, default=Path(os.environ.get("XDG_CACHE_HOME", str(Path.home() / ".cache"))) / "dsh-ocean/tools")
    args = parser.parse_args()
    try:
        output = fetch(args.url, args.sha256, Path(os.path.abspath(args.output_dir)))
        print(f"[appimagetool] verified non-executable cache: {output}")
    except (OSError, ValueError) as error:
        print(f"[appimagetool] failed: {error}. Existing cache outputs were not overwritten.", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
