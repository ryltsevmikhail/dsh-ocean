# Narrow, audited source backports

`glib/` retains the 121 published files of gtk-rs `glib 0.18.5`, with its original
MIT license and attribution. Only two lines implement upstream's
RUSTSEC-2024-0429 fix. This is not an Ocean-authored replacement for gtk-rs.

See [rationale, provenance, validation and scanner limits](../docs/GLIB_BACKPORT.md).
The adjacent provenance JSON and patch are not part of the upstream crate tree.
Do not regenerate or format that tree, change its version to silence a scanner,
or add unrelated edits. CI compares it with the checksum-pinned published archive.
