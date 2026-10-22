#!/usr/bin/env python3
"""Build public release metadata only. No signing, networking or publication."""
import argparse
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import re
import stat

REPOSITORY = "https://github.com/ryltsevmikhail/dsh-ocean/releases/download"
ARCHES = {"linux-x86_64": ("x86_64", "linux-x64"), "linux-aarch64": ("aarch64", "linux-arm64")}
NUMBER = r"(?:0|[1-9][0-9]*)"
IDENTIFIER = r"(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)"
VERSION = re.compile(rf"{NUMBER}\.{NUMBER}\.{NUMBER}(?:-{IDENTIFIER}(?:\.{IDENTIFIER})*)?\Z")
MAX_ASSET = 1024 * 1024 * 1024


def version(value):
    if not isinstance(value, str) or len(value) > 128 or not VERSION.fullmatch(value):
        raise ValueError("invalid canonical component version")
    if any(int(part) >= 2**64 for part in value.split("-", 1)[0].split(".")):
        raise ValueError("component version exceeds client integer bounds")
    return value


def read_json(path):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, "rb") as handle:
        info = os.fstat(handle.fileno())
        if not stat.S_ISREG(info.st_mode) or info.st_size > 65536:
            raise ValueError("metadata is not a bounded regular file")
        raw = handle.read(65537)
    if len(raw) > 65536:
        raise ValueError("metadata grew beyond limit")
    return json.loads(raw)


def components(args):
    if args.product == "shell":
        return None, None
    source = read_json(args.components)
    if source["schema"] != 1 or source["dsh"]["package"] != "@deepseek-ai/dsh":
        raise ValueError("unexpected runtime component source")
    values = {"node": version(source["node"]["version"]), "dsh": version(source["dsh"]["version"]), "pnpm": version(source["pnpm"]["version"])}
    if values["dsh"] != args.release:
        raise ValueError("runtime release differs from repository component pin")
    hashes = {}
    for target, (_, node_arch) in ARCHES.items():
        digest = source["node"]["archives"][node_arch]["sha256"]
        if not isinstance(digest, str) or not re.fullmatch("[a-f0-9]{64}", digest):
            raise ValueError("invalid pinned Node archive hash")
        hashes[target] = digest
    return values, hashes


def identity(args, target):
    arch = ARCHES[target][0]
    if args.product == "runtime":
        tag = f"runtime-{args.channel}-v{args.release}-r{args.sequence}"
        name = f"dsh-ocean-runtime-{args.channel}-{args.release}-{args.sequence}-{arch}.tar.gz"
    else:
        tag = f"v{args.release}"
        name = f"dsh-ocean-{args.release}-{arch}.tar.gz"
    return name, f"{REPOSITORY}/{tag}/{name}"


def describe(args):
    _, hashes = components(args)
    name, url = identity(args, args.target)
    if args.file.name != name:
        raise ValueError("artifact filename is not canonical")
    fd = os.open(args.file, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, "rb") as handle:
        before = os.fstat(handle.fileno())
        if not stat.S_ISREG(before.st_mode) or before.st_nlink != 1 or not 0 < before.st_size <= MAX_ASSET:
            raise ValueError("artifact is not a bounded regular file")
        digest = hashlib.sha256()
        count = 0
        while chunk := handle.read(65536):
            count += len(chunk)
            if count > before.st_size:
                raise ValueError("artifact grew during hashing")
            digest.update(chunk)
        after = os.fstat(handle.fileno())
        if count != before.st_size or (before.st_size, before.st_mtime_ns, before.st_ctime_ns) != (after.st_size, after.st_mtime_ns, after.st_ctime_ns):
            raise ValueError("artifact changed during hashing")
    result = {"target": args.target, "kind": "tar.gz", "url": url, "size": count, "sha256": digest.hexdigest()}
    if hashes is not None:
        result["nodeArchiveSha256"] = hashes[args.target]
    return result


def manifest(args):
    values, hashes = components(args)
    artifacts = []
    targets = set()
    fields = {"target", "kind", "url", "size", "sha256"} | ({"nodeArchiveSha256"} if hashes is not None else set())
    for path in args.artifact:
        item = read_json(path)
        if not isinstance(item, dict) or set(item) != fields:
            raise ValueError("artifact metadata has unexpected fields")
        target = item["target"]
        if target not in ARCHES or target in targets:
            raise ValueError("duplicate or unsupported artifact target")
        targets.add(target)
        _, url = identity(args, target)
        if item["kind"] != "tar.gz" or item["url"] != url or type(item["size"]) is not int or not 0 < item["size"] <= MAX_ASSET or not isinstance(item["sha256"], str) or not re.fullmatch("[a-f0-9]{64}", item["sha256"]):
            raise ValueError("artifact policy does not match this release")
        if hashes is not None and item["nodeArchiveSha256"] != hashes[target]:
            raise ValueError("artifact original Node archive differs from repository pin")
        name, _ = identity(args, target)
        actual_args = dict(vars(args))
        actual_args.update(target=target, file=path.parent / name)
        if describe(argparse.Namespace(**actual_args)) != item:
            raise ValueError("built archive bytes changed after artifact description")
        artifacts.append(item)
    if targets != set(ARCHES) and not (args.single_target_validation and len(targets) == 1):
        raise ValueError("publication manifest requires both native architecture artifacts")
    if not 1 <= args.valid_hours <= 720:
        raise ValueError("manifest lifetime exceeds client policy")
    now = dt.datetime.now(dt.timezone.utc).replace(microsecond=0)
    result = {"version": 1, "product": "dsh-ocean-runtime" if values is not None else "dsh-ocean", "channel": args.channel, "sequence": args.sequence, "release": args.release, "publishedAt": now.isoformat().replace("+00:00", "Z"), "expiresAt": (now + dt.timedelta(hours=args.valid_hours)).isoformat().replace("+00:00", "Z"), "artifacts": sorted(artifacts, key=lambda item: item["target"])}
    if values is not None:
        result.update(components=values, stateScope="web-profile-sessions-storages-v1")
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="operation", required=True)
    for name in ["plan", "artifact", "manifest"]:
        command = sub.add_parser(name)
        command.add_argument("--product", choices=["shell", "runtime"], required=True)
        command.add_argument("--channel", choices=["stable", "next"], required=True)
        command.add_argument("--release", required=True)
        command.add_argument("--sequence", required=True, type=int)
        command.add_argument("--components", type=Path, default=Path(__file__).resolve().parent.parent / "runtime-manifest.json")
        if name == "plan":
            command.add_argument("--source-commit")
            command.add_argument("--tag-commit")
        if name == "artifact":
            command.add_argument("--file", type=Path, required=True)
            command.add_argument("--target", choices=ARCHES, required=True)
        elif name == "manifest":
            command.add_argument("--artifact", type=Path, action="append", required=True)
            command.add_argument("--single-target-validation", action="store_true", help="temporary per-runner validation only; publication requires both targets by default")
            command.add_argument("--valid-hours", type=int, default=168)
    args = parser.parse_args()
    try:
        version(args.release)
        if args.channel == "stable" and "-" in args.release:
            raise ValueError("prerelease refused on stable channel")
        if not 0 < args.sequence < 2**64:
            raise ValueError("release sequence outside client policy")
        if args.operation == "plan":
            if args.source_commit is not None or args.tag_commit is not None:
                if not isinstance(args.source_commit, str) or not re.fullmatch(r"(?:[a-f0-9]{40}|[a-f0-9]{64})", args.source_commit) or args.source_commit != args.tag_commit:
                    raise ValueError("release tag does not resolve to the exact built source commit")
            values, _ = components(args)
            _, url = identity(args, "linux-x86_64")
            result = {"product": args.product, "channel": args.channel, "release": args.release, "sequence": args.sequence, "tag": url.rsplit("/", 1)[0].rsplit("/", 1)[1], "targets": sorted(ARCHES), "components": values, "published": False}
            if args.source_commit is not None:
                result["sourceCommit"] = args.source_commit
        else:
            result = describe(args) if args.operation == "artifact" else manifest(args)
        raw = json.dumps(result, separators=(",", ":"), sort_keys=True)
        if len(raw.encode()) > 65536:
            raise ValueError("release metadata exceeds client limit")
        print(raw)
    except (OSError, ValueError, TypeError, KeyError):
        parser.exit(1, "release metadata refused: check pinned components, canonical artifacts, sequence and targets\n")


if __name__ == "__main__":
    main()
