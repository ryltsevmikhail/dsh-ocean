#!/usr/bin/env python3
"""Trusted portable payload, new user targets only. No upgrade/service operation."""
import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import platform
import secrets
import stat
import sys
if sys.version_info < (3, 11):
    sys.exit("[install] Python 3.11+ is required only for installation, not running the client")


def stamp(info):
    return (info.st_dev, info.st_ino, info.st_size, info.st_mtime_ns, info.st_ctime_ns, info.st_mode)


def absolute(value):
    if not value or not Path(value).is_absolute() or any(ord(char) < 32 or ord(char) == 127 for char in value):
        raise ValueError("install paths must be absolute and contain no control characters")
    if ".." in Path(value).parts:
        raise ValueError("install paths must not contain parent traversal")
    return Path(os.path.abspath(value))


def desktop_exec(path):
    # Exec quoting is followed by desktop string-value escaping. %% represents
    # a literal percent, not a field code, even inside a quoted executable.
    argument = str(path).replace("%", "%%")
    argument = "".join("\\" + char if char in '\\"$`' else char for char in argument)
    return ('"' + argument + '"').replace("\\", "\\\\")


def parent_fd(path, create, created=None):
    """Open (and optionally create) the target's parent with no-follow steps.

    Created directory components are appended to ``created`` as absolute paths so
    a later uninstall can prune exactly what this installer made.
    """
    current = os.open("/", os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
    walked = []
    try:
        for part in path.parent.parts[1:]:
            walked.append(part)
            try:
                child = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=current)
            except FileNotFoundError:
                if not create:
                    os.close(current)
                    return None
                try:
                    os.mkdir(part, mode=0o700, dir_fd=current)
                except FileExistsError:
                    pass
                else:
                    if created is not None:
                        created.append(Path("/").joinpath(*walked))
                child = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=current)
            os.close(current)
            current = child
        info = os.fstat(current)
        if info.st_uid != os.geteuid() or info.st_mode & 0o022:
            raise ValueError("install target parent must be owned and not writable by others")
        return current
    except BaseException:
        os.close(current)
        raise


def exists(fd, name):
    try:
        os.stat(name, dir_fd=fd, follow_symlinks=False)
        return True
    except FileNotFoundError:
        return False


def same_parent(path, fd):
    info, held = path.parent.lstat(), os.fstat(fd)
    if not stat.S_ISDIR(info.st_mode) or (info.st_dev, info.st_ino) != (held.st_dev, held.st_ino):
        raise ValueError("install parent identity changed")


def owned_unlink(fd, name, owned):
    try:
        current = os.stat(name, dir_fd=fd, follow_symlinks=False)
    except FileNotFoundError:
        return
    if (current.st_dev, current.st_ino) != (owned.st_dev, owned.st_ino):
        raise ValueError("install output replaced; refusing cleanup")
    os.unlink(name, dir_fd=fd)


def source_root_fd(root):
    current = os.open("/", os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
    try:
        for part in root.parts[1:]:
            child = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=current)
            os.close(current)
            current = child
        return current
    except BaseException:
        os.close(current)
        raise


def same_source_root(root, fd):
    current, held = root.lstat(), os.fstat(fd)
    if not stat.S_ISDIR(current.st_mode) or (current.st_dev, current.st_ino) != (held.st_dev, held.st_ino):
        raise ValueError("install source root identity changed")


def source_file(root_fd, relative, binary=False):
    parent = os.dup(root_fd)
    try:
        parts = Path(relative).parts
        for part in parts[:-1]:
            child = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=parent)
            os.close(parent)
            parent = child
        fd = os.open(parts[-1], os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC, dir_fd=parent)
    finally:
        os.close(parent)
    source = os.fdopen(fd, "rb")
    try:
        before = os.fstat(source.fileno())
        if not stat.S_ISREG(before.st_mode) or before.st_nlink != 1 or before.st_size > (512 if binary else 16) * 1024 * 1024:
            raise ValueError("install source must be bounded, regular and singly linked")
        if binary:
            header = source.read(64)
            machine = {"x86_64": 62, "aarch64": 183}.get(platform.machine())
            if (machine is None or len(header) != 64 or header[:7] != b"\x7fELF\x02\x01\x01" or header[7] not in (0, 3)
                    or int.from_bytes(header[16:18], "little") not in (2, 3) or int.from_bytes(header[18:20], "little") != machine):
                raise ValueError("install source is not a matching native Linux ELF")
            source.seek(0)
        return source, before
    except BaseException:
        source.close()
        raise


def read_bounded(path, limit):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC)
    with os.fdopen(fd, "rb") as source:
        info = os.fstat(source.fileno())
        if not stat.S_ISREG(info.st_mode) or info.st_size > limit:
            raise ValueError("ledger is not a bounded regular file")
        data = source.read(limit + 1)
        if len(data) > limit:
            raise ValueError("ledger grew beyond its bound")
        return data


def hash_staged(fd):
    result = hashlib.sha256()
    os.lseek(fd, 0, os.SEEK_SET)
    while True:
        block = os.read(fd, 65536)
        if not block:
            return result.hexdigest()
        result.update(block)


def ledger_target(state_home):
    return state_home / "dsh-ocean/installed-portable.json"


def write_ledger(records, ledger, created_dirs=(), roots=()):
    fd = parent_fd(ledger, True, created_dirs)
    record = {"parent": fd, "target": ledger, "temporary": ".dsh-ocean-ledger-" + secrets.token_hex(16), "fd": None, "owned": None, "published": False}
    try:
        body = (json.dumps({
            "schema": 1,
            "files": [{"path": str(item["target"]), "mode": item["mode"], "size": item["size"], "sha256": item["sha256"]} for item in records],
            "directories": sorted(str(directory) for directory in created_dirs
                                  if any(root in directory.parents for root in roots)),
        }, sort_keys=True, indent=1) + "\n").encode()
        stage = os.open(record["temporary"], os.O_RDWR | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600, dir_fd=fd)
        record["fd"], record["owned"] = stage, os.fstat(stage)
        with os.fdopen(os.dup(stage), "wb") as output:
            output.write(body)
            output.flush()
            os.fchmod(stage, 0o600)
            os.fsync(stage)
        same_parent(ledger, fd)
        os.link(record["temporary"], ledger.name, src_dir_fd=fd, dst_dir_fd=fd, follow_symlinks=False)
        record["published"] = True
        published = os.stat(ledger.name, dir_fd=fd, follow_symlinks=False)
        if (published.st_dev, published.st_ino) != (record["owned"].st_dev, record["owned"].st_ino):
            raise ValueError("install ledger publication changed")
        return record
    except BaseException:
        try:
            if record["published"] and record["owned"] is not None:
                try:
                    owned_unlink(fd, ledger.name, record["owned"])
                except (OSError, ValueError):
                    pass
            if record["owned"] is not None:
                try:
                    owned_unlink(fd, record["temporary"], record["owned"])
                except (OSError, ValueError):
                    pass
        finally:
            if record["fd"] is not None:
                os.close(record["fd"])
            os.close(fd)
        raise


def uninstall(binary_dir, data_home, state_home):
    ledger = ledger_target(state_home)
    if platform.system() != "Linux" or os.geteuid() == 0:
        raise ValueError("portable uninstall requires a non-root Linux user")
    try:
        ledger_before = os.stat(ledger, follow_symlinks=False)
        raw = read_bounded(ledger, 64 * 1024)
        ledger_stamp = stamp(os.stat(ledger, follow_symlinks=False))
        if not stat.S_ISREG(ledger_before.st_mode) or ledger_stamp != stamp(ledger_before) or not ledger_before.st_nlink == 1:
            raise ValueError("install ledger is not a stable private regular file")
        document = json.loads(raw.decode("utf-8"))
        recorded_dirs = [absolute(str(entry)) for entry in document.get("directories", [])]
        if not all(any(root in directory.parents for root in (binary_dir, data_home, state_home)) for directory in recorded_dirs):
            raise ValueError("install ledger records a directory outside the owned prefixes")
    except FileNotFoundError:
        raise ValueError("no portable install ledger found; nothing to uninstall") from None
    files = document.get("files") if isinstance(document, dict) else None
    if not isinstance(document, dict) or document.get("schema") != 1 or not isinstance(files, list) or not files:
        raise ValueError("unsupported or malformed install ledger")
    roots = [binary_dir, data_home]
    entries, seen = [], set()
    for item in files:
        if not isinstance(item, dict) or set(item) != {"path", "mode", "size", "sha256"}:
            raise ValueError("malformed install ledger entry")
        path = absolute(str(item["path"]))
        if path in seen or not any(root in path.parents for root in roots):
            raise ValueError("install ledger path is outside the owned prefixes")
        if not isinstance(item["mode"], int) or not isinstance(item["size"], int) or not isinstance(item["sha256"], str) or not item["size"] >= 0 or not 0 <= item["mode"] <= 0o7777:
            raise ValueError("malformed install ledger entry")
        seen.add(path)
        entries.append((path, int(item["mode"]), int(item["size"]), item["sha256"]))
    verified, refused = [], []
    for path, mode, size, expected in entries:
        fd = parent_fd(path, False)
        if fd is None:
            continue  # already absent; do not invent ownership of the path
        try:
            try:
                current = os.stat(path.name, dir_fd=fd, follow_symlinks=False)
            except FileNotFoundError:
                continue
            # A crash between link and scratch-unlink leaves exactly one of our
            # own `.dsh-ocean-install-*` names pointing at the same inode.
            leftovers = []
            if stat.S_ISREG(current.st_mode) and current.st_nlink == 2:
                leftovers = [
                    name for name in os.listdir(fd)
                    if name.startswith(".dsh-ocean-install-")
                    and os.stat(name, dir_fd=fd, follow_symlinks=False).st_ino == current.st_ino
                    and os.stat(name, dir_fd=fd, follow_symlinks=False).st_dev == current.st_dev
                ]
            if (not stat.S_ISREG(current.st_mode) or current.st_nlink != 1 + len(leftovers)
                    or current.st_size != size or current.st_mode & 0o7777 != int(mode)):
                refused.append(str(path))
                continue
            actual = hashlib.sha256()
            reader = os.open(path.name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC, dir_fd=fd)
            with os.fdopen(reader, "rb") as source:
                before = os.fstat(source.fileno())
                if stamp(before) != stamp(current):
                    refused.append(str(path))
                    continue
                while True:
                    block = source.read(65536)
                    if not block:
                        break
                    actual.update(block)
                if stamp(os.fstat(source.fileno())) != stamp(before) or stamp(path.lstat()) != stamp(before):
                    refused.append(str(path))
                    continue
            if actual.hexdigest() != expected:
                refused.append(str(path))
                continue
            verified.append((path, stamp(current), leftovers))
        finally:
            os.close(fd)
    # Verify every recorded file before removing any: a refusal is non-destructive.
    if refused:
        raise ValueError("modified, replaced or non-private files were kept with the ledger intact: " + ", ".join(refused))
    removed = []
    for path, expected_stamp, leftovers in verified:
        fd = parent_fd(path, False)
        if fd is None:
            continue
        try:
            current = os.stat(path.name, dir_fd=fd, follow_symlinks=False)
            if stamp(current) != expected_stamp:
                raise ValueError("recorded file changed between verification and removal; ledger retained")
            os.unlink(path.name, dir_fd=fd)
            for name in leftovers:
                leftover = os.stat(name, dir_fd=fd, follow_symlinks=False)
                if (leftover.st_dev, leftover.st_ino) != (current.st_dev, current.st_ino):
                    raise ValueError("install scratch name was replaced; ledger retained")
                os.unlink(name, dir_fd=fd)
            os.fsync(fd)
            removed.append(str(path))
        finally:
            os.close(fd)
    # Prune only directories this installer recorded as newly created; shared
    # XDG trees that already existed (even if they ended up empty) are kept.
    for directory in sorted(recorded_dirs, key=lambda item: len(item.parts), reverse=True):
        if not any(root in directory.parents for root in roots):
            continue
        try:
            os.rmdir(directory)
        except OSError:
            pass
    fd = parent_fd(ledger, False)
    if fd is not None:
        try:
            current = os.stat(ledger.name, dir_fd=fd, follow_symlinks=False)
            if not stat.S_ISREG(current.st_mode) or stamp(current) != ledger_stamp:
                raise ValueError("install ledger changed during uninstall; it was kept for inspection")
            os.unlink(ledger.name, dir_fd=fd)
            os.fsync(fd)
        finally:
            os.close(fd)
    if ledger.parent in recorded_dirs:
        try:
            os.rmdir(ledger.parent)
        except OSError:
            pass
    return removed


def install(root, binary_dir, data_home, state_home, check=False):
    if platform.system() != "Linux" or os.geteuid() == 0:
        raise ValueError("portable installation requires a non-root Linux user")
    binary = binary_dir / "dsh-ocean"
    inputs = [("dsh-ocean", binary, 0o755)]
    inputs += [(f"icons/{size}x{size}.png", data_home / f"icons/hicolor/{size}x{size}/apps/dsh-ocean.png", 0o644) for size in (32, 64, 128, 256, 512)]
    inputs += [("packaging/dsh-ocean.desktop", data_home / "applications/dev.dsh.ocean.desktop", 0o644)]
    root = absolute(str(root))
    opened, records, created_dirs = [], [], []
    ledger_record = None
    root_fd = None
    committed = False
    try:
        root_fd = source_root_fd(root)
        for relative, target, mode in inputs:
            source, before = source_file(root_fd, relative, relative == "dsh-ocean")
            opened.append((source, before, root / relative, target, mode))
        desktop = opened[-1][0]
        lines = desktop.read(opened[-1][1].st_size + 1).decode("utf-8").splitlines()
        if sum(line.startswith("Exec=") for line in lines) != 1:
            raise ValueError("expected one desktop Exec field")
        content = ("\n".join("Exec=" + desktop_exec(binary) if line.startswith("Exec=") else line for line in lines) + "\n").encode()
        ledger = ledger_target(state_home)
        for _, _, _, target, _ in opened + [(None, None, None, ledger, None)]:
            fd = parent_fd(target, False)
            if fd is not None:
                try:
                    if exists(fd, target.name):
                        raise FileExistsError("install target exists; run --uninstall first (upgrades are not supported)")
                finally:
                    os.close(fd)
        if check:
            same_source_root(root, root_fd)
            for source, before, path, _, _ in opened:
                if stamp(os.fstat(source.fileno())) != stamp(before) or stamp(path.lstat()) != stamp(before):
                    raise ValueError("install input changed during preflight")
            return
        for index, (source, before, path, target, mode) in enumerate(opened):
            fd = parent_fd(target, True, created_dirs)
            record = {"parent": fd, "target": target, "temporary": ".dsh-ocean-install-" + secrets.token_hex(16), "fd": None, "owned": None, "published": False}
            records.append(record)
            stage = os.open(record["temporary"], os.O_RDWR | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600, dir_fd=fd)
            record["fd"], record["owned"] = stage, os.fstat(stage)
            source.seek(0)
            reader = io.BytesIO(content) if index == len(opened) - 1 else source
            remaining = len(content) if index == len(opened) - 1 else before.st_size
            with os.fdopen(os.dup(stage), "wb") as output:
                while remaining:
                    block = reader.read(min(remaining, 65536))
                    if not block:
                        raise ValueError("install input shrank")
                    output.write(block)
                    remaining -= len(block)
                output.flush()
                os.fchmod(stage, mode)
                os.fsync(stage)
            final = os.fstat(stage)
            record["mode"], record["size"] = final.st_mode & 0o7777, final.st_size
            record["sha256"] = hash_staged(stage)
        for source, before, path, _, _ in opened:
            if stamp(os.fstat(source.fileno())) != stamp(before) or stamp(path.lstat()) != stamp(before):
                raise ValueError("install input changed")
        same_source_root(root, root_fd)
        for record in records:
            fd, target = record["parent"], record["target"]
            same_parent(target, fd)
            current = os.stat(record["temporary"], dir_fd=fd, follow_symlinks=False)
            if (current.st_dev, current.st_ino) != (record["owned"].st_dev, record["owned"].st_ino):
                raise ValueError("install scratch identity changed")
            os.link(record["temporary"], target.name, src_dir_fd=fd, dst_dir_fd=fd, follow_symlinks=False)
            record["published"] = True
            published = os.stat(target.name, dir_fd=fd, follow_symlinks=False)
            if (published.st_dev, published.st_ino) != (record["owned"].st_dev, record["owned"].st_ino):
                raise ValueError("install publication changed")
            # Drop the scratch name immediately: at most this one already-published
            # file can carry a second link if the process dies right here.
            owned_unlink(fd, record["temporary"], record["owned"])
            record["temporary"] = None
            os.fsync(fd)
        # The ledger is published only after every file is durable, and it is
        # rolled back last so a failed rollback still leaves it usable.
        # Only directories strictly inside the owned roots are prunable later;
        # shared ancestors such as `~/.local/state` are created but never recorded
        # as ours to remove.
        ledger_record = write_ledger(records, ledger, created_dirs,
                                     (binary_dir, data_home, state_home))
        owned_unlink(ledger_record["parent"], ledger_record["temporary"], ledger_record["owned"])
        ledger_record["temporary"] = None
        os.fsync(ledger_record["parent"])
        committed = True
    finally:
        cleanup_errors = []
        # Files first, ledger last: an interrupted rollback must leave the ledger
        # so the remaining files can still be inspected or removed safely.
        for record in reversed(records):
            try:
                names = []
                if not committed and record["published"]:
                    names.append(record["target"].name)
                if record["temporary"] is not None and record["owned"] is not None:
                    names.append(record["temporary"])
                for name in names:
                    try:
                        owned_unlink(record["parent"], name, record["owned"])
                    except (OSError, ValueError) as error:
                        cleanup_errors.append(str(error))
                os.fsync(record["parent"])
            except (OSError, ValueError) as error:
                cleanup_errors.append(str(error))
            finally:
                if record["fd"] is not None:
                    os.close(record["fd"])
                os.close(record["parent"])
        if ledger_record is not None:
            try:
                if not committed and ledger_record["published"] and not cleanup_errors:
                    owned_unlink(ledger_record["parent"], ledger_record["target"].name, ledger_record["owned"])
                if ledger_record["temporary"] is not None:
                    owned_unlink(ledger_record["parent"], ledger_record["temporary"], ledger_record["owned"])
                os.fsync(ledger_record["parent"])
            except (OSError, ValueError) as error:
                cleanup_errors.append(str(error))
            finally:
                if ledger_record["fd"] is not None:
                    os.close(ledger_record["fd"])
                os.close(ledger_record["parent"])
        for source, *_ in opened:
            source.close()
        if root_fd is not None:
            os.close(root_fd)
        if cleanup_errors:
            raise ValueError("install cleanup incomplete; inspect the private prefix; replacements were not deleted")


def main():
    parser = argparse.ArgumentParser(description="New user targets only; no overwrite, upgrade, cache helper, launch or service operation.")
    parser.add_argument("--check", action="store_true", help="read-only source/target preflight; no directories or files created")
    parser.add_argument("--uninstall", action="store_true", help="remove only ledger-recorded, hash-matching files")
    args = parser.parse_args()
    try:
        home = absolute(os.environ["HOME"])
        binary_dir = absolute(os.environ.get("DSH_OCEAN_BIN_DIR") or str(home / ".local/bin"))
        data_home = absolute(os.environ.get("XDG_DATA_HOME") or str(home / ".local/share"))
        state_home = absolute(os.environ.get("XDG_STATE_HOME") or str(home / ".local/state"))
        if args.uninstall:
            if args.check:
                parser.error("--uninstall cannot be combined with --check")
            removed = uninstall(binary_dir, data_home, state_home)
            print(f"[install] removed {len(removed)} ledger-recorded files; ledger cleared; no service/cache helper or application touched")
        else:
            install(Path(__file__).parent, binary_dir, data_home, state_home, args.check)
            print("[install] preflight passed; nothing created" if args.check else "[install] new user files and private ledger published; no service/cache helper or application launched")
    except (OSError, ValueError, KeyError) as error:
        print(f"[install] failed: {error}. Existing targets were not overwritten; inspect new empty directories or incomplete cleanup after IO failure.", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
