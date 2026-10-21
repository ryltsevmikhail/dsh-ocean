"""Synthetic license payload for disposable packaging/install fixtures only."""
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
MANIFEST = (ROOT / "packaging/license-inputs.txt").read_bytes()
NAMES = tuple(line for line in MANIFEST.decode().splitlines() if line and not line.startswith("#"))


def populate(root):
    root = Path(root)
    manifest = root / "packaging/license-inputs.txt"
    manifest.parent.mkdir(parents=True, exist_ok=True)
    manifest.write_bytes(MANIFEST)
    for name in NAMES:
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        if not path.exists():
            path.write_bytes(("synthetic license fixture " + name + "\n").encode())
