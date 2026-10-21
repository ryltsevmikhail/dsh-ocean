# Binary license evidence and remaining release gates

**Status: evidence collected, not legal clearance or permission to publish a release.**
Ocean's original code is MIT. That does not relicense embedded Rust dependencies,
system libraries, downloaded runtimes, or an AppImage runtime. No whole-executable
MIT claim is made here. This document concerns the explicit package helpers, not
an uninspected alternative `cargo tauri bundle` output.

## Collected Rust evidence

The [inventory](third-party/inventory.json) records the conservative union of
`cargo metadata --locked` default-feature graphs for native Linux aarch64 and
x86_64. It records declared expressions, source versions/checksums, features,
target predicates, build/proc-macro context and notice evidence. This is **not an
exact linker SBOM**. Package target declarations include targets not necessarily
built into the executable; Cargo metadata filtering is not a full host/target
cross-compilation simulation.

[THIRD_PARTY_NOTICES](../THIRD_PARTY_NOTICES.txt) retains supplied license,
COPYRIGHT and NOTICE texts with package/evidence references, deduplicating only
identical bytes. Evidence is checked against checksum-pinned published crates;
six omitted-file cases use pinned supplier files or exact complete inline notices,
not invented copyright holders or a generic MIT template. The local GTK3 GLib
backport retains both its LICENSE and COPYRIGHT and is identified as modified.

Declared `OR`, `AND`, legacy slash expressions and embedded notices are not
silently reduced to one permissive keyword. Retaining offered texts is not a
legal approval or an automatic license election. Records requiring manual review
remain marked as such; review the actual distribution and obligations before
release. The `manual_review` field is expression triage, not artifact approval;
`false` does not clear any of the release gates. This is not an exhaustive audit of every source header/generated asset.

## MPL covered source availability

Five declared MPL-2.0 crate source archives accompany the notice payload:
`cssparser`, `cssparser-macros`, `dtoa-short`, `option-ext`, and `selectors` at the
versions/checksums recorded in the inventory. They contain the exact published
covered source form, not a promise that a moving upstream URL will remain alive.
The checker verifies the source against the published archive and refuses local
modifications not represented there. If code is modified, provide the actual
modified covered source and preserve required notices; do not ship an old archive.

The `.crate` files under `docs/third-party/sources/` are gzip-compressed tar source
archives. Copy an archive to an empty private directory and inspect/extract it with
a trusted archive tool; no build/install/service action is needed to obtain source.
The notices and inventory identify source paths and hashes. This does not relicense
Ocean's entire larger work under MPL. Keep this recipient-facing mechanism with
published packages and retain/review source availability for every actual release.

## What the helpers distribute

| Artifact | Deliberate payload / boundary |
|---|---|
| Portable tar | One Cargo-selected native ELF, original icons/art, fixed docs/installers, collected notices/inventory and covered-source archives. No loose Node/DSH/npm or GTK/WebKit library tree. |
| Debian | Same native ELF and fixed documentation under `/usr/share/doc/dsh-ocean`; third-party notice text is also included in the generated copyright file. System package dependencies are not a native-library license inventory. |
| AppDir | ELF, launcher/icons and the fixed documentation payload; helper does not run a library bundler. It still depends on the host ABI/GTK/WebKit unless independently demonstrated otherwise. |
| Genuine AppImage | **Still a gate:** inspect the final image and any embedded runtime/library copies. Pinning appimagetool does not pin/identify all runtime downloads or their licenses/source obligations. Stub/AppDir tests are not genuine-image acceptance. |

The shared reviewed [payload list](../packaging/license-inputs.txt) is explicit,
not a recursive copy of build/user directories. Portable and source installation
retain it with notices/source under `$XDG_DATA_HOME/doc/dsh-ocean/`; deleting an
extracted tar must not delete the user's only license copy. Portable uninstall
uses its ownership ledger and preserves modified notice files. The Arch template
also retains notices and documents; its mixed-license placeholder and source
checksum template require distribution-specific review, not blind AUR publication.

## Artifact-specific observations

The source checkout now provides a separate [toolchain/native evidence guide](https://github.com/ryltsevmikhail/dsh-ocean/blob/main/docs/TOOLCHAIN_NATIVE_AUDIT.md)
and bounded non-executing ELF report collector. CI retains the exact binary hash,
observed compiler context and public notice candidates as report-only artifacts.
These companions are not installed license payloads or build attestations, and do
not clear any of the coverage/redistribution gates below.

## Not yet cleared by this inventory

Before a binary release, the maintainer must record evidence for the **exact
commit, compiler, target, features and final artifact**, including:

- Rust std, compiler-builtins, unwinding/backtrace/compiler runtime and associated
  licenses/exceptions: these are not fully represented by the application's Cargo.lock.
- Fresh ELF interpreter/DT_NEEDED and native closure/dlopen behavior, target distro
  package origins/versions and actual copied versus system-provided libraries.
  A direct `readelf` list is not a transitive/dlopen inventory. Do not infer that
  all native libraries are MIT, or that absent direct SSL means no indirect SSL.
- For native copies actually redistributed, applicable LGPL/GPL/source/replacement,
  attribution and exception obligations. Keeping libraries dynamic is not by
  itself a complete legal review. Do not overwrite users' system libraries.
- Actual AppImage runtime/embedded-file inventory and license/source provenance;
  supported distro/ABI/package-name matrix (including Debian t64 differences).
- Original-art/contributor authority and any non-Cargo embedded assets/scripts.
  An origin assertion is evidence to review, not a chain-of-title certification.
- Other distribution/update paths (raw ELF copying, `cargo install`, a future
  signed shell-update feed) must carry matching notices and covered source for
  the actual version. This change covers the listed initial-install helpers,
  not automatic license-payload replacement by every updater.
- Separate later downloads: bootstrap fetches Node, DSH, pnpm, npm transitives and
  possible native build products. They are not in these ELF package payloads.
  Preserve upstream licenses; audit them if mirroring/rebundling generations.

No runtime/provider/profile is inspected by the source inventory. Nothing here
signs, publishes, installs or updates the working desktop/server. Packaging smoke
and complete notice presence are necessary checks, not final release authorization.

## Regeneration and checks

```bash
python3 -B scripts/license-inventory.py
python3 -B scripts/license-inventory.py --check
python3 -B -m unittest discover -s tests -p 'test_license_inventory.py' -v
python3 -B -m unittest discover -s tests -p 'test_package_*.py' -v
python3 -B -m unittest discover -s tests -p 'test_source_install.py' -v
```

Generation reads public locked Cargo sources/archives and exact pinned supplier
evidence. Missing or changed evidence must fail, not be guessed or silently skipped.
Refresh after dependency/feature/manifest changes, review the diff and all unresolved
records, then validate packaging and a fresh installation only in a disposable
HOME/XDG fixture. Independent review and an explicit maintainer decision are still
required before any release.
