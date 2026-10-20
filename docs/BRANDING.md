# Ocean Ice identity

The project uses the original **Ice** (Лёд) mark selected for Ocean: two flowing
shapes form an abstract O. It is not the upstream DeepSeek whale/favicon.
The master geometry was taken directly from the maintainer-selected vector design,
not reconstructed from a raster screenshot.

## Master and palette

- Vector master: [ocean-ice.svg](../assets/ocean-ice.svg).
- Coordinate system: `viewBox="0 0 1024 1024"`, square, transparent background.
- First current: blue **#2375D8** (RGB 35, 117, 216).
- Second current: ice **#B8DFF5** (RGB 184, 223, 245).
- Keep the original approximately 10% clear space, aspect ratio and unmodified
  curves. Do not stretch, crop, add gradients/shadows or substitute upstream marks.
- The pale current has low contrast on white; check the whole mark at the intended
  size/background rather than relying on that pale shape alone as a UI indicator.

## Application assets

`assets/dsh-app-icon.svg`, both favicon SVGs and `ui/dsh-icon.svg` contain the master
artwork. Legacy filenames are kept for compatibility; they do not identify the
artwork as belonging to DSH upstream.

PNG sizes are 32, 64, 128, 256 (`128x128@2x`) and 512 pixels. The ICO contains 16,
32, 64, 128 and 256-pixel variants. Small sizes are rasterized directly from the
vector, not enlarged from another PNG.

Tray artwork uses the same paths in one color: **#F1F8FC** for dark themes and
**#172A43** for light themes. `tray-dark` means *for a dark background*, not dark
foreground. Test real panel visibility on the desktop; theme detection is not a
guarantee that every panel uses the application's theme.

## Reproduce

Install trusted distribution packages providing librsvg's `rsvg-convert` and
ImageMagick 7's `magick`, then run from the checkout:

```bash
python3 -B scripts/render-icons.py
```

The helper performs no downloads and is not invoked by application startup or
installers. Review regenerated files before committing. The master, palette,
shipped dimensions and monochrome derivatives are covered by offline tests.

## License and relationship

Ocean artwork is provided under the project's [MIT license](../LICENSE), with
copyright attributed to DSH Ocean contributors. Third-party dependencies retain
their own licenses: see [NOTICE](../NOTICE). Ocean is an independent community
project, not an official DeepSeek product. Compatibility naming does not confer
trademark rights, endorsement or an exclusive right to generic ocean/O imagery.
