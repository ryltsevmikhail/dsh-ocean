#!/usr/bin/env python3
"""Publisher-only complete runtime archive. No npm/network/key/service operations."""
import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import tarfile
import tempfile

NUMBER = r"(?:0|[1-9][0-9]*)"
PRERELEASE = r"(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)"
VERSION = re.compile(rf"{NUMBER}\.{NUMBER}\.{NUMBER}(?:-{PRERELEASE}(?:\.{PRERELEASE})*)?\Z")
MAX_FILE = 512 * 1024 * 1024
MAX_TOTAL = 2 * 1024 * 1024 * 1024
BIN_ENTRIES = {"harness/node_modules/.bin/dsh", "tools/node_modules/.bin/pnpm"}


class Refused(ValueError):
    """Only fixed publisher validation messages; never attach paths or payloads."""


def regular(path):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    info = os.fstat(fd)
    if not stat.S_ISREG(info.st_mode) or info.st_size > MAX_FILE:
        os.close(fd)
        raise Refused("publisher input is not a bounded regular file")
    return os.fdopen(fd, "rb")


def node_entry(path):
    if path.suffix in {".js", ".cjs", ".mjs"}:
        return True
    if path.suffix:
        return False
    # npm bin targets may be extensionless (e.g. @anthropic-ai/sdk/bin/cli).
    # Do not reinterpret native/shell/unknown entrypoints as Node programs.
    with regular(path) as handle:
        first_line = handle.read(128).split(b"\n", 1)[0]
    return first_line in {b"#!/usr/bin/env node", b"#!/usr/bin/node"}


def build(args):
    source = args.root.absolute()
    if source.is_symlink() or not source.is_dir():
        raise Refused("runtime root must be a real directory")
    source = source.resolve(strict=True)
    if not VERSION.fullmatch(args.release) or not 0 < args.sequence < 2**64:
        raise Refused("invalid runtime release/sequence")
    with regular(source / ".versions") as handle:
        versions = handle.read(4097)
    if len(versions) > 4096:
        raise Refused("component record exceeds limit")
    components = versions.decode("ascii").splitlines()
    if versions != ("\n".join(components) + "\n").encode("ascii") or len(components) != 4 or not all(VERSION.fullmatch(value) for value in components[:3]) or components[1] != args.release or not re.fullmatch("[0-9a-f]{64}", components[3]):
        raise Refused("runtime component/source record mismatch")
    output = args.output_dir.absolute()
    if output == source or source in output.parents or output in source.parents:
        raise Refused("publisher output must be outside runtime source")
    output.mkdir(parents=True, exist_ok=True, mode=0o700)
    arch = args.target.removeprefix("linux-")
    root_name = f"dsh-ocean-runtime-{args.channel}-{args.release}-{args.sequence}-linux-{arch}"
    filename = f"dsh-ocean-runtime-{args.channel}-{args.release}-{args.sequence}-{arch}.tar.gz"
    destination = output / filename
    if destination.exists() or destination.is_symlink() or (output / "artifact.json").exists():
        raise Refused("publisher output already exists")
    total = 0
    entries = 0
    with tempfile.TemporaryDirectory(prefix=".runtime-package-", dir=output) as temporary:
        stage = Path(temporary) / root_name
        stage.mkdir(mode=0o700)
        pending = [Path(".versions"), Path("runtime"), Path("harness"), Path("tools")]
        for name in ["NOTICE", "LICENSE", "THIRD_PARTY_NOTICES.txt", "THIRD_PARTY_NOTICES"]:
            if (source / name).exists() or (source / name).is_symlink():
                pending.append(Path(name))
        while pending:
            relative = pending.pop()
            entries += 1
            if entries > 200000 or len(relative.as_posix()) > 4096 or len(relative.parts) > 128:
                raise Refused("runtime payload exceeds tree bounds")
            current = source / relative
            target = stage / relative
            info = current.lstat()
            # The checksum-pinned Node distribution ships this EMPTY builtin
            # npm config. Preserve only its proven empty bytes, never a user,
            # project, symlinked or credential-bearing npm configuration.
            if relative == Path("runtime/node/lib/node_modules/npm/.npmrc") and stat.S_ISREG(info.st_mode) and not info.st_mode & 0o7000:
                with regular(current) as handle:
                    if handle.read(1):
                        raise Refused("publisher payload contains credential/manager-only input")
                target.parent.mkdir(parents=True, exist_ok=True)
                with target.open("xb"):
                    pass
                target.chmod(0o644)
                continue
            if relative.name in {".npmrc", ".env", ".previous", ".ocean-runtime.json", ".ocean-runtime-files.json"}:
                raise Refused("publisher payload contains credential/manager-only input")
            if info.st_mode & 0o7000:
                raise Refused("special permission bits refused")
            target.parent.mkdir(parents=True, exist_ok=True)
            if stat.S_ISLNK(info.st_mode):
                resolved = current.resolve(strict=True)
                if source not in resolved.parents or not resolved.is_file():
                    raise Refused("runtime link leaves source or points to a directory")
                bin_entry = relative.parent.name == ".bin" or relative.parent == Path("runtime/node/bin")
                if not bin_entry or not node_entry(resolved):
                    raise Refused("publisher must resolve non-JS-entry symlinks explicitly")
                entry = os.path.relpath(resolved, current.parent)
                # execve replaces this Node process in-place: correct package-relative
                # imports/main argv and no wrapper process that can orphan a server.
                shim = "#!/usr/bin/env node\nconst path=require('node:path');const entry=path.resolve(__dirname," + json.dumps(entry) + ");if(typeof process.execve!=='function')throw new Error('pinned Node execve required');process.execve(process.execPath,[process.execPath,entry,...process.argv.slice(2)],process.env);\n"
                target.write_text(shim, encoding="utf-8")
                target.chmod(0o755)
                scope = target.parent / "package.json"
                scope.write_text('{"type":"commonjs"}\n', encoding="utf-8")
            elif stat.S_ISDIR(info.st_mode):
                target.mkdir(exist_ok=True)
                target.chmod(0o755)
                pending.extend(relative / child.name for child in current.iterdir())
            elif stat.S_ISREG(info.st_mode):
                total += info.st_size
                if total > MAX_TOTAL:
                    raise Refused("runtime payload exceeds total bound")
                with regular(current) as handle, target.open("xb") as writer:
                    before = os.fstat(handle.fileno())
                    while chunk := handle.read(65536):
                        writer.write(chunk)
                    after = os.fstat(handle.fileno())
                    if (before.st_size, before.st_mtime_ns, before.st_ctime_ns) != (after.st_size, after.st_mtime_ns, after.st_ctime_ns):
                        raise Refused("publisher input changed during copy")
                target.chmod(0o755 if info.st_mode & 0o111 else 0o644)
            else:
                raise Refused("runtime special file refused")
        for name in ["runtime/node/bin/node", *BIN_ENTRIES]:
            if not (stage / name).is_file() or (stage / name).stat().st_size == 0:
                raise Refused("complete runtime entry missing")
            (stage / name).chmod(0o755)
        staged_archive = Path(temporary) / "runtime.tar.gz"
        with staged_archive.open("xb") as raw, gzip.GzipFile(fileobj=raw, mode="wb", filename="", mtime=0) as compressed, tarfile.open(fileobj=compressed, mode="w", format=tarfile.GNU_FORMAT) as archive:
            for path in [stage, *sorted(stage.rglob("*"))]:
                name = root_name if path == stage else root_name + "/" + path.relative_to(stage).as_posix()
                item = archive.gettarinfo(str(path), arcname=name)
                item.uid = item.gid = 0
                item.uname = item.gname = ""
                item.mtime = 0
                if item.isfile():
                    with path.open("rb") as content:
                        archive.addfile(item, content)
                else:
                    archive.addfile(item)
        digest = hashlib.sha256()
        with staged_archive.open("rb") as content:
            while chunk := content.read(65536):
                digest.update(chunk)
        if staged_archive.stat().st_size > 1024 * 1024 * 1024:
            raise Refused("compressed runtime exceeds client bound")
        metadata = {"target": args.target, "kind": "tar.gz", "url": f"https://github.com/ryltsevmikhail/dsh-ocean/releases/download/runtime-{args.channel}-v{args.release}-r{args.sequence}/{filename}", "size": staged_archive.stat().st_size, "sha256": digest.hexdigest(), "nodeArchiveSha256": components[3]}
        # Hard-link publication is atomic and refuses existing outputs.
        os.link(staged_archive, destination)
        with (output / "artifact.json").open("x", encoding="utf-8") as handle:
            json.dump(metadata, handle, sort_keys=True)
            handle.write("\n")
    return metadata


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", required=True, type=Path)
    parser.add_argument("--output-dir", required=True, type=Path)
    parser.add_argument("--channel", choices=["stable", "next"], required=True)
    parser.add_argument("--release", required=True)
    parser.add_argument("--sequence", type=int, required=True)
    parser.add_argument("--target", choices=["linux-x86_64", "linux-aarch64"], required=True)
    args = parser.parse_args()
    try:
        print(json.dumps(build(args), sort_keys=True))
    except Refused as error:
        parser.exit(1, f"runtime package refused: {error}\n")
    except (OSError, ValueError, UnicodeError, tarfile.TarError):
        parser.exit(1, "runtime package refused: verify complete publisher inputs, versions and safe paths\n")


if __name__ == "__main__":
    main()
