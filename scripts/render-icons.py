#!/usr/bin/env python3
"""Render Ocean Ice assets from the selected original SVG (no runtime downloads).

Requires librsvg's rsvg-convert and ImageMagick 7's magick. This is a maintainer
asset-generation helper, not part of application startup or package installation.
"""
import shutil
import subprocess
import tempfile
from pathlib import Path
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[1]
SVG = "http://www.w3.org/2000/svg"
ET.register_namespace("", SVG)


def main():
    for tool in ("rsvg-convert", "magick"):
        if not shutil.which(tool):
            raise SystemExit(f"Install {tool} before rendering icons")
    source = ROOT / "assets/ocean-ice.svg"
    tree = ET.parse(source)
    root = tree.getroot()
    paths = root.findall(f"{{{SVG}}}path")
    if root.get("viewBox") != "0 0 1024 1024" or len(paths) != 2:
        raise SystemExit("Expected the reviewed two-path 1024-square Ice master")
    if [path.get("fill") for path in paths] != ["#2375D8", "#B8DFF5"]:
        raise SystemExit("Unexpected Ice master palette")
    primary = source.read_text()
    for relative in ("assets/dsh-app-icon.svg", "assets/dsh-favicon.svg",
                     "assets/dsh-favicon-dark.svg", "ui/dsh-icon.svg"):
        (ROOT / relative).write_text(primary)
    for name, color in (("dark", "#F1F8FC"), ("light", "#172A43")):
        for path in paths:
            path.set("fill", color)
        (ROOT / f"assets/dsh-tray-{name}.svg").write_text(
            ET.tostring(root, encoding="unicode") + "\n")
    for name, size in (("32x32", 32), ("64x64", 64), ("128x128", 128),
                       ("128x128@2x", 256), ("icon", 512)):
        subprocess.run(["rsvg-convert", "--width", str(size), "--height", str(size),
                        "--output", str(ROOT / f"icons/{name}.png"), str(source)], check=True)
    for name in ("dark", "light"):
        subprocess.run(["rsvg-convert", "--width", "32", "--height", "32",
                        "--output", str(ROOT / f"icons/tray-{name}.png"),
                        str(ROOT / f"assets/dsh-tray-{name}.svg")], check=True)
    with tempfile.TemporaryDirectory(prefix="ocean-ice-icons-") as directory:
        small = Path(directory) / "16.png"
        subprocess.run(["rsvg-convert", "--width", "16", "--height", "16",
                        "--output", str(small), str(source)], check=True)
        subprocess.run(["magick", str(small), str(ROOT / "icons/32x32.png"),
                        str(ROOT / "icons/64x64.png"), str(ROOT / "icons/128x128.png"),
                        str(ROOT / "icons/128x128@2x.png"), str(ROOT / "icons/icon.ico")], check=True)
    print("Rendered Ice SVG, PNG, ICO and light/dark tray assets")


if __name__ == "__main__":
    main()
