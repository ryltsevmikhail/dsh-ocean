#!/usr/bin/env python3
"""AppImage builder with explicit tool pin; trusted tool is not sandboxed."""
import argparse
import os
from pathlib import Path
import re
import runpy
import shutil
import subprocess
import sys
import tempfile


def stage_inputs(appdir, opened, epoch):
    appdir.mkdir(mode=0o755)
    for key, (source, before, _, _) in opened.items():
        if key in ("install.sh", "install.py"):
            continue
        if key == "dsh-ocean":
            relative = "usr/bin/dsh-ocean"
        elif key == "packaging/dsh-ocean.desktop":
            relative = "usr/share/applications/dsh-ocean.desktop"
        elif key.startswith("icons/"):
            size = key.split("/")[1].removesuffix(".png")
            relative = f"usr/share/icons/hicolor/{size}/apps/dsh-ocean.png"
        elif key in ("assets/ocean-ice.svg", "packaging/license-inputs.txt", "locales/dsh/LICENSE") or key in ("README.md", "CHANGELOG.md", "LICENSE", "NOTICE", "THIRD_PARTY_NOTICES.txt", "CONTRIBUTING.md", "SECURITY.md", "SUPPORT.md", "CODE_OF_CONDUCT.md", "GOVERNANCE.md") or key.startswith("docs/"):
            relative = "usr/share/doc/dsh-ocean/" + key
        else:
            raise ValueError("unknown AppImage payload input")
        target = appdir / relative
        target.parent.mkdir(mode=0o755, parents=True, exist_ok=True)
        source.seek(0)
        with target.open("xb") as output:
            remaining = before.st_size
            while remaining:
                block = source.read(min(remaining, 65536))
                if not block:
                    raise ValueError("AppImage input shrank")
                output.write(block)
                remaining -= len(block)
        target.chmod(0o755 if key == "dsh-ocean" else 0o644)
    desktop = appdir / "usr/share/applications/dsh-ocean.desktop"
    lines = desktop.read_bytes().splitlines()
    if sum(line.startswith(b"Exec=") for line in lines) != 1 or sum(line.startswith(b"Icon=") for line in lines) != 1:
        raise ValueError("expected one desktop Exec and Icon")
    content = b"\n".join(b"Exec=dsh-ocean" if line.startswith(b"Exec=") else b"Icon=dsh-ocean" if line.startswith(b"Icon=") else line for line in lines) + b"\n"
    desktop.write_bytes(content)
    (appdir / "dsh-ocean.desktop").write_bytes(content)
    shutil.copyfile(appdir / "usr/share/icons/hicolor/512x512/apps/dsh-ocean.png", appdir / "dsh-ocean.png")
    (appdir / "AppRun").write_text('#!/bin/sh\nHERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)\nexec "$HERE/usr/bin/dsh-ocean" "$@"\n')
    (appdir / "AppRun").chmod(0o755)
    # The private parent protects staging; squashfs members must be readable by
    # the eventual user, independent of the build user's umask.
    for directory, dirs, files in os.walk(appdir):
        Path(directory).chmod(0o755)
        os.utime(directory, (epoch, epoch))
        for name in files:
            path = Path(directory) / name
            path.chmod(0o755 if path == appdir / "AppRun" or path == appdir / "usr/bin/dsh-ocean" else 0o644)
            os.utime(path, (epoch, epoch))


def writer(tool, workspace, backend):
    def build(output, version, arch, opened, epoch):
        appdir = workspace / "DSH-Ocean.AppDir"
        stage_inputs(appdir, opened, epoch)
        extraction = workspace / "tool-extraction"
        extraction.mkdir(mode=0o700)
        subprocess.run([str(tool), "--appimage-extract"], cwd=extraction, check=True, stdout=subprocess.DEVNULL, timeout=120)
        extracted_root = extraction / "squashfs-root"
        if extracted_root.is_symlink() or not extracted_root.is_dir():
            raise ValueError("invalid extracted tool root")
        extracted_root.resolve(strict=True).relative_to(extraction.resolve(strict=True))
        entry = extracted_root / "AppRun"
        entry.resolve(strict=True).relative_to(extracted_root.resolve(strict=True))
        if not entry.is_file() or not os.access(entry, os.X_OK):
            raise ValueError("invalid extracted appimagetool entry")
        image = workspace / "new-output.AppImage"
        subprocess.run([str(entry), "--no-appstream", str(appdir), str(image)], cwd=workspace,
                       env={**os.environ, "APPDIR": str(extracted_root), "ARCH": arch, "SOURCE_DATE_EPOCH": str(epoch)}, check=True, timeout=300)
        source, before = backend["open_source"](image, binary=True, machine={"x86_64": 62, "aarch64": 183}[arch])
        with source:
            header = source.read(64)
            if header[8:11] != b"AI\x02":
                raise ValueError("output is not a type-2 AppImage header")
            source.seek(0)
            remaining = before.st_size
            while remaining:
                block = source.read(min(remaining, 65536))
                if not block:
                    raise ValueError("AppImage output shrank")
                output.write(block)
                remaining -= len(block)
            if backend["stamp"](os.fstat(source.fileno())) != backend["stamp"](before) or backend["stamp"](image.lstat()) != backend["stamp"](before):
                raise ValueError("AppImage output changed while reading")
    return build


def main():
    root = Path(__file__).resolve().parent.parent
    parser = argparse.ArgumentParser(description="Build with explicitly pinned appimagetool; never trust an implicit cached/continuous executable.")
    parser.add_argument("--tool", type=Path, required=True)
    parser.add_argument("--tool-sha256", required=True)
    parser.add_argument("--output-dir", type=Path, default=root / "dist")
    args = parser.parse_args()
    epoch = os.environ.get("SOURCE_DATE_EPOCH", "0")
    if len(epoch) > 10 or not re.fullmatch(r"[0-9]+", epoch) or int(epoch) > 0xffffffff:
        parser.error("SOURCE_DATE_EPOCH must be an unsigned 32-bit integer")
    try:
        backend = runpy.run_path(str(root / "scripts/package-tar.py"))
        tools = runpy.run_path(str(root / "scripts/appimagetool.py"))
        with tempfile.TemporaryDirectory(prefix="dsh-ocean-appimage-") as temporary:
            workspace = Path(temporary)
            tool = tools["snapshot_tool"](args.tool, workspace / "verified-tool", args.tool_sha256)
            result = backend["package"](root, Path(os.path.abspath(args.output_dir)), int(epoch),
                archive_writer=writer(tool, workspace, backend), archive_name=lambda version, arch: f"dsh-ocean-{version}-{arch}.AppImage")
            print(f"[appimage] {result}")
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        print(f"[appimage] failed: {error}. Existing outputs were not overwritten; inspect any newly published output after sync/cleanup failure.", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
