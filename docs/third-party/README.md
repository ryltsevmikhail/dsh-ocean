# Focused Cargo source-license inventory

This is a practical bounded source-license audit, **not legal certification**, exact linked-binary SBOM, or final release approval. Root project terms remain in LICENSE and NOTICE; dependency terms are not replaced by those terms.

## Reproduce / check

Python 3.11+ standard library and the locked public Cargo source/cache archives are required. From the repository root:

```sh
python3 scripts/license-inventory.py
python3 scripts/license-inventory.py --check
```

Without inputs, the script runs `cargo metadata --locked --format-version 1 --filter-platform TARGET` for both `aarch64-unknown-linux-gnu` and `x86_64-unknown-linux-gnu`, from the root with default features (no feature overrides). It does not build anything. Cargo may fetch locked public dependencies. Populate a missing archive cache explicitly with Cargo; the collector does not download supplier evidence or extract/write cache source trees.

For externally captured metadata from those exact commands and the current manifest/lock:

```sh
python3 scripts/license-inventory.py \
  --metadata aarch64-unknown-linux-gnu=/tmp/arm.json \
  --metadata x86_64-unknown-linux-gnu=/tmp/x86.json --check
```

Both inputs are required together. Metadata is a trusted build-tool input, not independently authenticated compilation evidence; do not supply metadata generated with different feature arguments. Package identity is checked against the lock, but metadata cannot prove its invocation flags. `--check` regenerates in memory and compares bytes, including manifest/lock digests; it never rewrites the generated outputs.

Generated files: `docs/third-party/inventory.json` and root `THIRD_PARTY_NOTICES.txt`. No generated timestamp, absolute cache/home path, or current Git HEAD hash is included. All discovered LICENSE/LICENCE/COPYING/COPYRIGHT/NOTICE-like files are recursively retained, with text SHA deduplication and every package reference preserved. The checker verifies every registry archive against Cargo.lock and compares cached license bytes with those published archive members. It fails on missing/changed evidence, unsupported sources, symlinks, extra evidence/source filenames, or size/traversal limits. Limits include 2MB per text, 16MB cumulative text, 16MB per registry archive, 128MB expanded per archive, 30,000 entries per package/archive, and 5,000 metadata packages. These are explicit fail-closed limits, not claims that larger inputs are unsafe or unlicensed.

Incoming edge kinds are recorded separately from propagated conservative normal/build/proc-macro graph contexts. Context propagation is a source-graph superset, not Cargo's precise host/target compilation or linked-membership analysis. Original license expressions and all supplied terms are preserved. OR/legacy slash expressions and embedded third-party notices require manual review; the script never selects a branch or declares a legal approval. Embedded terms may exist outside license-like filenames; full source review remains a release gate.

## Supplier evidence and MPL covered source

`evidence/index.json` records exact pinned supplier URLs, SHA256, package references and excerpt lines. The index itself is pinned by the collector: edits need a deliberate reviewed code/index change. The six omitted standalone files are supplemented without inventing attribution. qrcodegen preserves its complete original MIT header, lines 1–22; selectors preserves its source header and separately receives full MPL 2.0 terms from the exact cached/published cssparser LICENSE. That supplement is explicitly identified, not presented as an upstream selectors standalone file.

Only these five exact published Cargo.lock-checksummed MPL covered-source archives are bundled:

- `sources/cssparser-0.37.0.crate`
- `sources/cssparser-macros-0.7.1.crate`
- `sources/dtoa-short-0.3.5.crate`
- `sources/option-ext-0.2.0.crate`
- `sources/selectors-0.38.0.crate`

Recipients can extract each using `tar -xzf NAME-VERSION.crate`; the resulting `NAME-VERSION/` contains the complete published source and preserved headers, not merely a remote availability link. The package allowlist carries these archives alongside notices/inventory and the lead's binary-audit document. This is not a whole-application MPL relicense. If covered sources are modified in future, replace the release payload with corresponding complete modified covered source and review obligations; do not distribute stale published archives as if they covered modifications.

The local glib backport is represented honestly as a path dependency at exactly `vendor/glib`; both COPYRIGHT and LICENSE are included. Its source availability points at the fixed public `86bb883` tree, and its upstream archive/fix and patched-file evidence are recorded from the existing provenance manifest. The collector pins the provenance blob and validates the full 121-file tree (with the exact patched-file hash), not only the license or modified lines. Semantic backport correctness and optimized behavior still have separate review/regression checks.

## Still gated

Rust std/compiler-builtins/unwind; system/native/dlopen libraries; real AppImage contents/runtime; later Node/pnpm/bootstrap downloads; embedded third-party code not covered by filename discovery; original art attribution and contributor chain-of-title. This inventory does not certify these, nor claim the entire ELF is MIT. Future lock/manifest/feature/source updates require regeneration and human review. Source archives must remain available to executable recipients through the package/install payload, not just a development checkout.
