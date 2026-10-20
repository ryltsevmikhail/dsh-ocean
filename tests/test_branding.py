"""Offline contracts for the selected Ocean Ice master and shipped derivatives."""
from pathlib import Path
import struct
import unittest
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[1]
NS = "http://www.w3.org/2000/svg"


def svg(path):
    root = ET.parse(ROOT / path).getroot()
    assert root.tag == f"{{{NS}}}svg"
    assert root.get("viewBox") == "0 0 1024 1024"
    assert all(node.tag in {f"{{{NS}}}{name}" for name in ("svg", "title", "path")}
               for node in root.iter())
    assert all("href" not in key for node in root.iter() for key in node.attrib)
    return root.findall(f"{{{NS}}}path")


class Branding(unittest.TestCase):
    def test_master_is_selected_two_current_palette(self):
        paths = svg("assets/ocean-ice.svg")
        self.assertEqual(len(paths), 2)
        self.assertEqual([path.get("fill") for path in paths], ["#2375D8", "#B8DFF5"])
        self.assertTrue(all(path.get("d") for path in paths))

    def test_application_and_favicon_vectors_preserve_master(self):
        master = [(path.get("d"), path.get("fill")) for path in svg("assets/ocean-ice.svg")]
        for name in ("assets/dsh-app-icon.svg", "assets/dsh-favicon.svg",
                     "assets/dsh-favicon-dark.svg", "ui/dsh-icon.svg"):
            with self.subTest(name=name):
                self.assertEqual([(p.get("d"), p.get("fill")) for p in svg(name)], master)

    def test_tray_preserves_geometry_with_theme_specific_single_color(self):
        master = [path.get("d") for path in svg("assets/ocean-ice.svg")]
        for theme, color in (("dark", "#F1F8FC"), ("light", "#172A43")):
            paths = svg(f"assets/dsh-tray-{theme}.svg")
            self.assertEqual([path.get("d") for path in paths], master)
            self.assertEqual([path.get("fill") for path in paths], [color, color])

    def test_shipped_png_dimensions_and_alpha_channel(self):
        for name, size in (("32x32", 32), ("64x64", 64), ("128x128", 128),
                           ("128x128@2x", 256), ("icon", 512),
                           ("tray-dark", 32), ("tray-light", 32)):
            with self.subTest(name=name):
                data = (ROOT / f"icons/{name}.png").read_bytes()
                self.assertEqual(data[:8], b"\x89PNG\r\n\x1a\n")
                self.assertEqual(data[12:16], b"IHDR")
                self.assertEqual(struct.unpack(">II", data[16:24]), (size, size))
                self.assertEqual(data[25], 6)  # RGBA, not a flattened background.

    def test_ico_contains_small_and_large_sizes_with_valid_bounds(self):
        data = (ROOT / "icons/icon.ico").read_bytes()
        reserved, kind, count = struct.unpack_from("<HHH", data)
        self.assertEqual((reserved, kind, count), (0, 1, 5))
        dimensions = []
        for index in range(count):
            width, height, _, _, _, _, size, offset = struct.unpack_from("<BBBBHHII", data, 6 + index * 16)
            dimensions.append((width or 256, height or 256))
            self.assertGreater(size, 0)
            self.assertGreaterEqual(offset, 6 + count * 16)
            self.assertLessEqual(offset + size, len(data))
        self.assertEqual(dimensions, [(n, n) for n in (16, 32, 64, 128, 256)])


if __name__ == "__main__":
    unittest.main()
