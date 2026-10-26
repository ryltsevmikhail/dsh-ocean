#!/usr/bin/env python3
"""Host-native Debian archive, no install/maintainer scripts. Shared locked inputs."""
import argparse
import gzip
import hashlib
import io
import os
from pathlib import Path, PurePosixPath
import re
import runpy
import shutil
import subprocess
import sys
import tarfile
import tempfile


ARCHITECTURES = {"x86_64": "amd64", "aarch64": "arm64"}


def deb_version(version):
    upstream, plus, build = version.partition("+")
    upstream = upstream.replace("-", "~", 1)
    # Explicit Debian revision allows upstream prerelease/build hyphens without
    # accidentally treating one of them as the package revision separator.
    return upstream + (plus + build if plus else "") + "-1"


def filename(version, arch):
    return f"dsh-ocean_{deb_version(version)}_{ARCHITECTURES[arch]}.deb"


def destinations(opened):
    result = {}
    for key in opened:
        if key in ("install.sh", "install.py"):
            continue  # dpkg payload must never run/include portable installer.
        if key == "dsh-ocean":
            path = "usr/bin/dsh-ocean"
        elif key == "packaging/dsh-ocean.desktop":
            path = "usr/share/applications/dev.dsh.ocean.desktop"
        elif key in ("assets/ocean-ice.svg", "packaging/license-inputs.txt", "locales/dsh/LICENSE") or key in ("README.md", "CHANGELOG.md", "LICENSE", "NOTICE", "THIRD_PARTY_NOTICES.txt", "CONTRIBUTING.md", "SECURITY.md", "SUPPORT.md", "CODE_OF_CONDUCT.md", "GOVERNANCE.md") or key.startswith("docs/"):
            path = "usr/share/doc/dsh-ocean/" + key
        elif key.startswith("icons/"):
            size = key.split("/")[1].removesuffix(".png")
            path = f"usr/share/icons/hicolor/{size}/apps/dsh-ocean.png"
        else:
            raise ValueError("unknown Debian payload input")
        if PurePosixPath(path).is_absolute() or ".." in PurePosixPath(path).parts or path in result:
            raise ValueError("invalid/duplicate Debian payload path")
        result[path] = key
    result["usr/share/doc/dsh-ocean/copyright"] = None
    return result


class HashReader:
    def __init__(self, source):
        self.source = source
        self.hash = hashlib.md5(usedforsecurity=False)  # dpkg convention, not trust.

    def read(self, size):
        block = self.source.read(size)
        self.hash.update(block)
        return block


def write_data(raw, opened, epoch):
    mapping = destinations(opened)
    parents = {str(parent) for path in mapping for parent in PurePosixPath(path).parents if str(parent) != "."}
    checksums = []
    installed_kib = 0
    with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=epoch) as compressed:
        with tarfile.open(fileobj=compressed, mode="w", format=tarfile.PAX_FORMAT) as archive:
            for path in sorted(parents):
                entry = tarfile.TarInfo(path + "/")
                entry.type, entry.mode, entry.mtime = tarfile.DIRTYPE, 0o755, epoch
                archive.addfile(entry)
            for path in sorted(mapping):
                key = mapping[path]
                if key is None:
                    notices = []
                    for license_name in ("LICENSE", "NOTICE", "THIRD_PARTY_NOTICES.txt"):
                        source, info, _, _ = opened[license_name]
                        source.seek(0)
                        content = source.read(info.st_size + 1)
                        if len(content) != info.st_size:
                            raise ValueError("package license input changed")
                        notices.append(content)
                    content = b"DSH Ocean project license and upstream notices\n\n" + b"\n\n".join(notices) + b"\n"
                    source, size = io.BytesIO(content), len(content)
                else:
                    source, info, _, _ = opened[key]
                    source.seek(0)
                    size = info.st_size
                if key == "packaging/dsh-ocean.desktop":
                    lines = source.read(info.st_size + 1).splitlines()
                    if sum(line.startswith(b"Exec=") for line in lines) != 1:
                        raise ValueError("expected one desktop Exec entry")
                    content = b"\n".join(b"Exec=/usr/bin/dsh-ocean" if line.startswith(b"Exec=") else line for line in lines) + b"\n"
                    source, size = io.BytesIO(content), len(content)
                entry = tarfile.TarInfo(path)
                entry.size, entry.mode, entry.mtime = size, 0o755 if key == "dsh-ocean" else 0o644, epoch
                reader = HashReader(source)
                archive.addfile(entry, reader)
                checksums.append(f"{reader.hash.hexdigest()}  {path}\n")
                installed_kib += (size + 1023) // 1024
    return "".join(checksums).encode(), installed_kib


def write_control(raw, version, arch, checksums, installed_kib, epoch):
    control = f"""Package: dsh-ocean
Version: {deb_version(version)}
Section: devel
Priority: optional
Architecture: {ARCHITECTURES[arch]}
Maintainer: DSH Ocean contributors <noreply@example.invalid>
Depends: curl, ca-certificates, libgtk-3-0, libwebkit2gtk-4.1-0, libayatana-appindicator3-1, libc6, libgcc-s1
Recommends: nodejs
Installed-Size: {installed_kib}
Description: DSH Ocean - native desktop client for DeepSeek Harness
 A thin Tauri shell around the official DeepSeek Harness UI.
 Host-built system-WebKit package; no bundled GTK/WebKit or ABI guarantee.
""".encode()
    with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=epoch) as compressed:
        with tarfile.open(fileobj=compressed, mode="w", format=tarfile.PAX_FORMAT) as archive:
            for name, content in [("control", control), ("md5sums", checksums)]:
                entry = tarfile.TarInfo(name)
                entry.size, entry.mode, entry.mtime = len(content), 0o644, epoch
                archive.addfile(entry, io.BytesIO(content))


def ar_member(output, name, source, epoch):
    size = source.seek(0, os.SEEK_END)
    if size > 1024 * 1024 * 1024:
        raise ValueError("Debian archive member exceeds size bound")
    source.seek(0)
    header = f"{name + '/':<16}{epoch:<12}{0:<6}{0:<6}{'100644':<8}{size:<10}`\n".encode("ascii")
    if len(header) != 60:
        raise ValueError("invalid Debian ar header")
    output.write(header)
    shutil.copyfileobj(source, output, length=65536)
    if size % 2:
        output.write(b"\n")


def write_deb(output, version, arch, opened, epoch):
    # Anonymous/private intermediates only, no reused stage or recursive cleanup.
    with tempfile.TemporaryFile("w+b") as data, tempfile.TemporaryFile("w+b") as control:
        checksums, installed_kib = write_data(data, opened, epoch)
        write_control(control, version, arch, checksums, installed_kib, epoch)
        output.write(b"!<arch>\n")
        ar_member(output, "debian-binary", io.BytesIO(b"2.0\n"), epoch)
        ar_member(output, "control.tar.gz", control, epoch)
        ar_member(output, "data.tar.gz", data, epoch)


def main():
    root = Path(__file__).resolve().parent.parent
    parser = argparse.ArgumentParser(description="Build a fresh private Debian archive; no installation. Python 3.11+ and trusted Cargo required.")
    parser.add_argument("--output-dir", type=Path, default=root / "dist")
    args = parser.parse_args()
    epoch = os.environ.get("SOURCE_DATE_EPOCH", "0")
    if len(epoch) > 10 or not re.fullmatch(r"[0-9]+", epoch) or int(epoch) > 0xffffffff:
        parser.error("SOURCE_DATE_EPOCH must be an unsigned 32-bit integer")
    try:
        backend = runpy.run_path(str(root / "scripts/package-tar.py"))
        output = backend["package"](root, Path(os.path.abspath(args.output_dir)), int(epoch), archive_writer=write_deb, archive_name=filename)
        print(f"[deb] {output}")
    except (OSError, ValueError, KeyError, subprocess.SubprocessError, tarfile.TarError) as error:
        print(f"[deb] failed: {error}. No existing output was overwritten; inspect any newly published output after a sync/cleanup error.", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
