#!/usr/bin/env python3
"""Linux tar builder. Trusted Cargo executes normally; this is not a sandbox/signature."""
import argparse
import gzip
import json
import os
from pathlib import Path
import platform
import re
import secrets
import stat
import subprocess
import sys
import tarfile
if sys.version_info < (3, 11):
    sys.exit("[package] Python 3.11+ is required for the build helper (not the installed client)")
import tomllib

DOCS = ("README", "USAGE", "BRANDING", "OPEN_SOURCE_CHECKLIST", "ARCHITECTURE", "DOWNLOADS", "INSTALLATION", "NOTIFICATIONS", "PACKAGING", "PROFILE_BACKUPS", "RELEASE_VERIFICATION", "RELEASING", "ROADMAP", "RUNTIME_RELEASES", "SHELL_UPDATES", "UPDATE_TESTING", "TOOLCHAIN_NATIVE_AUDIT", "RELEASE_NATIVE_REVIEW")
INPUTS = {"README.md": "README.md", "CHANGELOG.md": "CHANGELOG.md", "LICENSE": "LICENSE", "NOTICE": "NOTICE", "install.sh": "packaging/install-from-package.sh", "install.py": "packaging/install-from-package.py", "packaging/dsh-ocean.desktop": "packaging/dsh-ocean.desktop"}
INPUTS.update({name: name for name in ("CONTRIBUTING.md", "SECURITY.md", "SUPPORT.md", "CODE_OF_CONDUCT.md", "GOVERNANCE.md")})
INPUTS.update({f"docs/{name}.md": f"docs/{name}.md" for name in DOCS})
INPUTS["assets/ocean-ice.svg"] = "assets/ocean-ice.svg"
INPUTS.update({f"icons/{size}x{size}.png": f"icons/{source}.png" for size, source in [(32,"32x32"),(64,"64x64"),(128,"128x128"),(256,"128x128@2x"),(512,"icon")]})


def stamp(info):
    return (info.st_dev, info.st_ino, info.st_size, info.st_mtime_ns, info.st_ctime_ns, info.st_mode)


def cargo_binary(root):
    command = ["cargo", "build", "--manifest-path", str(root / "Cargo.toml"), "--bin", "dsh-ocean", "--release", "--locked", "--message-format=json-render-diagnostics"]
    process = subprocess.run(command, cwd=root, stdout=subprocess.PIPE, text=True, check=False)
    artifacts = []
    for line in process.stdout.splitlines():
        row = json.loads(line)
        if not isinstance(row, dict) or not isinstance(row.get("target", {}), dict):
            raise ValueError("invalid Cargo JSON event shape")
        if row.get("reason") == "compiler-message":
            rendered = row.get("message", {}).get("rendered")
            if rendered:
                print(rendered, file=sys.stderr, end="")
        if (row.get("reason") == "compiler-artifact" and row.get("target", {}).get("name") == "dsh-ocean"
                and "bin" in row.get("target", {}).get("kind", []) and row.get("executable")
                and Path(row.get("manifest_path", "")).absolute() == root / "Cargo.toml"):
            artifacts.append(Path(row["executable"]).absolute())
    if process.returncode:
        raise subprocess.CalledProcessError(process.returncode, command)
    if len(artifacts) != 1:
        raise ValueError("Cargo did not report exactly one root client executable")
    return artifacts[0]


def open_source(path, binary=False, machine=None, repository=None):
    flags = os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW | os.O_NONBLOCK
    if repository is None:
        fd = os.open(path, flags)
    else:
        parts = path.relative_to(repository).parts
        parent = os.open(repository, os.O_RDONLY | os.O_CLOEXEC | os.O_DIRECTORY | os.O_NOFOLLOW)
        try:
            for part in parts[:-1]:
                child = os.open(part, os.O_RDONLY | os.O_CLOEXEC | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=parent)
                os.close(parent)
                parent = child
            fd = os.open(parts[-1], flags, dir_fd=parent)
        finally:
            os.close(parent)
    source = os.fdopen(fd, "rb")
    try:
        info = os.fstat(source.fileno())
        if not stat.S_ISREG(info.st_mode) or info.st_size > (512 if binary else 16) * 1024 * 1024:
            raise ValueError("package input is not a bounded regular file")
        if binary:
            header = source.read(64)
            if (not info.st_mode & 0o111 or len(header) < 64 or header[:7] != b"\x7fELF\x02\x01\x01"
                    or header[7] not in (0, 3) or int.from_bytes(header[16:18], "little") not in (2, 3)
                    or int.from_bytes(header[18:20], "little") != machine):
                raise ValueError("Cargo executable is not a matching native 64-bit Linux ELF")
            source.seek(0)
        return source, info
    except BaseException:
        source.close()
        raise


# Exact current covered-source set; dependency/source revisions require review.
LICENSE_BASE = {"LICENSE", "NOTICE", "THIRD_PARTY_NOTICES.txt", "locales/dsh/LICENSE", "docs/third-party/inventory.json", "docs/BINARY_LICENSE_AUDIT.md",
                "docs/third-party/sources/cssparser-0.37.0.crate", "docs/third-party/sources/cssparser-macros-0.7.1.crate",
                "docs/third-party/sources/dtoa-short-0.3.5.crate", "docs/third-party/sources/option-ext-0.2.0.crate",
                "docs/third-party/sources/selectors-0.38.0.crate"}

# Exact retained release-toolchain profile; not arbitrary notice paths.
LICENSE_BASE |= {'docs/toolchain/rust-1.90.0/index.json',
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


def license_payload(root):
    source, before = open_source(root / "packaging/license-inputs.txt", repository=root)
    with source:
        if before.st_size > 65536:
            raise ValueError("license payload manifest exceeds bound")
        raw = source.read(65537)
        if len(raw) > 65536 or stamp(os.fstat(source.fileno())) != stamp(before):
            raise ValueError("license payload manifest changed")
    names = []
    for name in raw.decode("utf-8").splitlines():
        if not name or name.startswith("#"):
            continue
        if name != name.strip() or (name not in LICENSE_BASE and not re.fullmatch(r"docs/third-party/sources/[a-z0-9][a-z0-9_-]*-[0-9][0-9A-Za-z._+-]*\.crate", name)):
            raise ValueError("unsupported license payload path")
        if name in names:
            raise ValueError("duplicate license payload path")
        names.append(name)
    if len(names) > 64 or set(names) != LICENSE_BASE:
        raise ValueError("required license payload missing")
    return tuple(names), raw


# One explicit reviewed list, never a recursive sweep of user/build directories.
LICENSE_INPUTS, LICENSE_MANIFEST = license_payload(Path(__file__).resolve().parents[1])
INPUTS.update({name: name for name in LICENSE_INPUTS})
INPUTS["packaging/license-inputs.txt"] = "packaging/license-inputs.txt"


def same_directory(path, fd):
    current = path.lstat()
    held = os.fstat(fd)
    if not stat.S_ISDIR(current.st_mode) or (current.st_dev, current.st_ino) != (held.st_dev, held.st_ino):
        raise ValueError("package output directory identity changed")


def package(root, directory, epoch, *, archive_writer=None, archive_name=None):
    manifest_path = root / "Cargo.toml"
    source, manifest_stamp = open_source(manifest_path, repository=root)
    with source:
        raw_manifest = source.read(16 * 1024 * 1024 + 1)
        if len(raw_manifest) > 16 * 1024 * 1024:
            raise ValueError("package manifest grew beyond its input bound")
        metadata = tomllib.loads(raw_manifest.decode("utf-8"))["package"]
        if stamp(os.fstat(source.fileno())) != stamp(manifest_stamp):
            raise ValueError("package manifest changed while reading")
    if not isinstance(metadata, dict):
        raise ValueError("invalid client package manifest table")
    version = metadata.get("version")
    if metadata.get("name") != "dsh-ocean" or not isinstance(version, str) or not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?", version):
        raise ValueError("invalid client package identity/version")
    arch = platform.machine()
    machines = {"x86_64": 62, "aarch64": 183}
    if platform.system() != "Linux" or arch not in machines:
        raise ValueError("unsupported native Linux package architecture")
    name = f"dsh-ocean-{version}-linux-{arch}"
    filename = archive_name(version, arch) if archive_name else f"{name}.tar.gz"
    if not isinstance(filename, str) or not filename or len(filename.encode()) > 255 or filename in (".", "..") or "/" in filename or "\\" in filename:
        raise ValueError("package output must be one bounded filename")
    output = directory / filename
    # lexists also refuses a dangling symlink, never replacing its referent.
    if os.path.lexists(output):
        raise FileExistsError("package output already exists; choose a new output directory")
    if directory.is_symlink():
        raise ValueError("package output directory must not be a symlink")
    binary = cargo_binary(root)  # Always ask Cargo, even for a fresh/cached executable.
    if stamp(manifest_path.lstat()) != stamp(manifest_stamp):
        raise ValueError("package manifest changed during Cargo build")
    inputs = {key: (root / value, 0o755 if key == "install.sh" else 0o644) for key, value in INPUTS.items()}
    inputs["dsh-ocean"] = (binary, 0o755)
    # Validate before creating output/scratch files, and keep input descriptors
    # through the archive write so paths cannot silently redirect a later open.
    opened = {}
    fd = None
    temporary = None
    temporary_identity = None
    pinned_scratch = None
    try:
        for key, (path, mode) in inputs.items():
            opened[key] = (*open_source(path, key == "dsh-ocean", machines[arch], root if key != "dsh-ocean" else None), path, mode)
        manifest_source = opened["packaging/license-inputs.txt"][0]
        if manifest_source.read(65537) != LICENSE_MANIFEST:
            raise ValueError("license payload manifest changed after selection")
        manifest_source.seek(0)
        directory.mkdir(mode=0o700, exist_ok=True)
        fd = os.open(directory, os.O_RDONLY | os.O_CLOEXEC | os.O_DIRECTORY | os.O_NOFOLLOW)
        info = os.fstat(fd)
        if info.st_uid != os.geteuid() or info.st_mode & 0o022:
            raise ValueError("package output directory must be owned and not writable by other users")
        same_directory(directory, fd)
        temporary = ".dsh-ocean-package-" + secrets.token_hex(16)
        scratch_fd = os.open(temporary, os.O_RDWR | os.O_CREAT | os.O_EXCL | os.O_CLOEXEC | os.O_NOFOLLOW, 0o600, dir_fd=fd)
        pinned_scratch = scratch_fd
        os.fchmod(scratch_fd, 0o600)
        temporary_identity = os.fstat(scratch_fd)
        with os.fdopen(os.dup(scratch_fd), "w+b") as raw:
            if archive_writer is None:
                with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=epoch) as compressed:
                    with tarfile.open(fileobj=compressed, mode="w", format=tarfile.PAX_FORMAT) as archive:
                        for rel in ["", "docs", "icons", "packaging"]:
                            entry = tarfile.TarInfo(name + ("/" + rel if rel else "") + "/")
                            entry.type = tarfile.DIRTYPE
                            entry.mode = 0o755
                            entry.mtime = epoch
                            archive.addfile(entry)
                        for key in sorted(opened):
                            source, before, path, mode = opened[key]
                            entry = tarfile.TarInfo(name + "/" + key)
                            entry.size, entry.mode, entry.mtime = before.st_size, mode, epoch
                            archive.addfile(entry, source)
            else:
                archive_writer(raw, version, arch, opened, epoch)
            for source, before, path, _ in opened.values():
                if stamp(os.fstat(source.fileno())) != stamp(before) or stamp(path.lstat()) != stamp(before):
                    raise ValueError("package input changed while archiving")
            raw.flush()
            os.fsync(raw.fileno())
        same_directory(directory, fd)
        current = os.stat(temporary, dir_fd=fd, follow_symlinks=False)
        if (current.st_dev, current.st_ino) != (temporary_identity.st_dev, temporary_identity.st_ino):
            raise ValueError("package scratch identity changed")
        if stamp(manifest_path.lstat()) != stamp(manifest_stamp):
            raise ValueError("package manifest changed before publication")
        os.link(temporary, output.name, src_dir_fd=fd, dst_dir_fd=fd, follow_symlinks=False)
        published = os.stat(output.name, dir_fd=fd, follow_symlinks=False)
        current = os.stat(temporary, dir_fd=fd, follow_symlinks=False)
        expected = (temporary_identity.st_dev, temporary_identity.st_ino)
        if (published.st_dev, published.st_ino) != expected or (current.st_dev, current.st_ino) != expected:
            raise ValueError("package publication/scratch identity changed; no cleanup performed on replacements")
        # Only our exclusive scratch inode may be removed; never any old output.
        os.unlink(temporary, dir_fd=fd)
        temporary = None
        os.fsync(fd)
        same_directory(directory, fd)
        return output
    finally:
        for source, *_ in opened.values():
            source.close()
        if fd is not None:
            try:
                if temporary is not None and temporary_identity is not None:
                    current = os.stat(temporary, dir_fd=fd, follow_symlinks=False)
                    if (current.st_dev, current.st_ino) == (temporary_identity.st_dev, temporary_identity.st_ino):
                        os.unlink(temporary, dir_fd=fd)
            finally:
                try:
                    os.close(fd)
                finally:
                    if pinned_scratch is not None:
                        os.close(pinned_scratch)


def main():
    root = Path(__file__).resolve().parent.parent
    parser = argparse.ArgumentParser(description="Build a fresh, private, no-overwrite native tar. Requires Python 3.11+ and a trusted Cargo/toolchain.")
    parser.add_argument("--output-dir", type=Path, default=root / "dist")
    args = parser.parse_args()
    value = os.environ.get("SOURCE_DATE_EPOCH", "0")
    if len(value) > 10 or not re.fullmatch(r"[0-9]+", value) or int(value) > 0xffffffff:
        parser.error("SOURCE_DATE_EPOCH must be an unsigned 32-bit integer")
    try:
        output = package(root, Path(os.path.abspath(args.output_dir)), int(value))
        print(f"[package] {output}")
    except (OSError, ValueError, KeyError, subprocess.SubprocessError, tarfile.TarError) as error:
        print(f"[package] failed: {error}. No existing output was overwritten; inspect any newly published output after a sync/cleanup error.", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
