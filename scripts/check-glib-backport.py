#!/usr/bin/env python3
"""Offline source/selection verification; optional pristine archive verification.

No downloads, Cargo-cache writes, source regeneration or advisory suppression.
"""
import argparse
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import stat
import tarfile
import tomllib

ARCHIVE_SHA256 = "233daaf6e83ae6a12a52055f568f9d7cf4671dabb78ff9560ab6da230ce00ee5"
ARCHIVE_URL = "https://static.crates.io/crates/glib/glib-0.18.5.crate"
BASE_COMMIT = "42b9caf98e03ded086362d9653ca58fe94dc8658"
FIX_COMMIT = "05dff0ee696f9bcd8617cd48c4b812d046d440cb"
PATCHED_FILE = "src/variant_iter.rs"
CHANGES = (
    (b"let p: *mut libc::c_char = std::ptr::null_mut();",
     b"let mut p: *mut libc::c_char = std::ptr::null_mut();"),
    (b"                &p,\n", b"                &mut p,\n"),
)


def require(condition, message):
    if not condition:
        raise ValueError(message)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def regular(path, limit=8 * 1024 * 1024):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, "rb") as stream:
        info = os.fstat(stream.fileno())
        require(stat.S_ISREG(info.st_mode) and info.st_size <= limit, "expected bounded regular file")
        data = stream.read(limit + 1)
    require(len(data) <= limit, "file grew past bound")
    return data


def safe_relative(value):
    path = PurePosixPath(value)
    require(bool(path.parts) and not path.is_absolute() and ".." not in path.parts,
            "unsafe provenance path")
    require(str(path) == value, "noncanonical provenance path")
    return Path(*path.parts)


def validate(root, archive=None):
    root = Path(root).resolve()
    vendor = root / "vendor" / "glib"
    for directory in (root / "vendor", vendor, root / "vendor/patches"):
        require(stat.S_ISDIR(directory.lstat().st_mode), "vendor directory must not be a symlink")
    provenance = json.loads(regular(root / "vendor/glib-0.18.5.provenance.json"))
    require(provenance["crate"] == "glib" and provenance["version"] == "0.18.5", "wrong crate baseline")
    require(provenance["archive_sha256"] == ARCHIVE_SHA256 and provenance["archive_url"] == ARCHIVE_URL,
            "wrong published archive identity")
    require(provenance["upstream_source_commit"] == BASE_COMMIT
            and provenance["upstream_fix_commit"] == FIX_COMMIT, "wrong upstream commits")
    require(provenance["upstream_fix_pr"] == "https://github.com/gtk-rs/gtk-rs-core/pull/1343",
            "wrong upstream fix reference")
    require(provenance["advisories"] == ["RUSTSEC-2024-0429", "GHSA-wrw7-89jp-8q8g"], "wrong advisory identity")
    require(provenance["patched_file"] == PATCHED_FILE, "unexpected patched path")
    original = provenance["original_files"]
    require(isinstance(original, dict) and len(original) == 121 and "LICENSE" in original,
            "incomplete published crate inventory")
    for name, checksum in original.items():
        safe_relative(name)
        require(isinstance(checksum, str) and len(checksum) == 64
                and all(c in "0123456789abcdef" for c in checksum), "invalid source checksum")
    seen = set()
    for directory, dirs, files in os.walk(vendor, followlinks=False):
        for name in dirs:
            require(not (Path(directory) / name).is_symlink(), "symlink in vendored source")
        for name in files:
            path = Path(directory) / name
            relative = path.relative_to(vendor).as_posix()
            require(relative in original, "unexpected vendored file: " + relative)
            data = regular(path)
            expected = provenance["patched_sha256"] if relative == PATCHED_FILE else original[relative]
            require(digest(data) == expected, "source checksum mismatch: " + relative)
            seen.add(relative)
    require(seen == set(original), "missing vendored source/license files")
    patched = regular(vendor / PATCHED_FILE)
    restored = patched
    for old, new in CHANGES:
        require(restored.count(new) == 1, "upstream mutability fix missing or ambiguous")
        restored = restored.replace(new, old)
    require(digest(restored) == original[PATCHED_FILE], "backport differs from exact upstream change")
    import difflib
    expected_patch = "".join(difflib.unified_diff(
        restored.decode().splitlines(True), patched.decode().splitlines(True),
        fromfile="a/src/variant_iter.rs", tofile="b/src/variant_iter.rs"))
    require(regular(root / "vendor/patches/glib-0.18.5-variant-str-iter.patch").decode() == expected_patch,
            "recorded patch differs from vendored source")
    cargo = tomllib.loads(regular(root / "Cargo.toml").decode())
    require(cargo["patch"]["crates-io"]["glib"] == {"path": "vendor/glib"}, "root Cargo override missing or changed")
    require(cargo["package"].get("publish") is False, "root must not publish a crate that discards its patch")
    lock = tomllib.loads(regular(root / "Cargo.lock").decode())
    glibs = [p for p in lock["package"] if p["name"] == "glib"]
    require(len(glibs) == 1 and glibs[0]["version"] == "0.18.5"
            and "source" not in glibs[0] and "checksum" not in glibs[0], "lockfile must select only the local patched glib")
    if archive is not None:
        raw = regular(Path(archive))
        require(digest(raw) == ARCHIVE_SHA256, "published archive checksum mismatch")
        inventory = {}
        with tarfile.open(fileobj=io.BytesIO(raw), mode="r:gz") as tar:
            for item in tar.getmembers():
                path = PurePosixPath(item.name)
                require(path.parts and path.parts[0] == "glib-0.18.5", "wrong archive root")
                safe_relative(str(path))
                if item.isdir():
                    continue
                require(item.isfile() and item.size < 8 * 1024 * 1024, "unsupported archive member")
                relative = str(PurePosixPath(*path.parts[1:]))
                require(relative not in inventory, "duplicate archive member")
                inventory[relative] = digest(tar.extractfile(item).read())
        require(inventory == original, "provenance does not match the published archive")
    return len(seen)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive", type=Path, help="optional locally supplied pristine .crate; never downloaded here")
    args = parser.parse_args()
    try:
        count = validate(Path(__file__).resolve().parents[1], args.archive)
    except (OSError, ValueError, KeyError, TypeError, tarfile.TarError) as error:
        parser.exit(1, f"GLib backport verification failed: {error}\n")
    print(f"GLib 0.18.5: {count} upstream files verified; only the exact two-line security backport differs.")
    print("Cargo selects the local patch. This does not dismiss a version-based advisory.")


if __name__ == "__main__":
    main()
