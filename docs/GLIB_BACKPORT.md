# GTK3-compatible GLib iterator security backport

## Issue and decision

[RUSTSEC-2024-0429](https://rustsec.org/advisories/RUSTSEC-2024-0429.html)
([GHSA-wrw7-89jp-8q8g](https://github.com/advisories/GHSA-wrw7-89jp-8q8g))
affects the Rust `glib` binding versions `>=0.15.0, <0.20.0`. `VariantStrIter`
passes an immutable reference to an out-pointer into variadic
`g_variant_get_child`; optimized Rust may disregard that write and subsequently
call `CStr::from_ptr(NULL)`. This is a binding-source defect, not a system GLib
package-version check.

Tauri/Wry, GTK3, GIO and WebKitGTK currently share the `glib 0.18` Rust type family.
Simply adding `glib 0.20` would leave their vulnerable dependency in the graph and
introduce incompatible Rust wrapper types. Upstream's `0.18` branch still points
to the `0.18.5` release baseline; no compatible fixed registry version was found.

Ocean therefore selects a local **0.18.5 + exact source backport**, using the root
`[patch.crates-io]`. It does **not** invent an unaffected version, mutate Cargo's
registry cache, add a parallel 0.20 binding, or dismiss the GitHub alert.

## Provenance and maintenance

- Published archive: `https://static.crates.io/crates/glib/glib-0.18.5.crate`
- Archive SHA-256 (also the former registry lock checksum):
  `233daaf6e83ae6a12a52055f568f9d7cf4671dabb78ff9560ab6da230ce00ee5`
- Original gtk-rs-core commit: `42b9caf98e03ded086362d9653ca58fe94dc8658`.
- Upstream fix: [PR 1343](https://github.com/gtk-rs/gtk-rs-core/pull/1343),
  merge commit `05dff0ee696f9bcd8617cd48c4b812d046d440cb`.
- All 121 published files are retained in `vendor/glib`, including its MIT
  [LICENSE](../vendor/glib/LICENSE), headers and original Cargo metadata.
- The **only** modification inside that tree is `src/variant_iter.rs`:
  `let p` becomes `let mut p`, and the out-argument `&p` becomes `&mut p`.
- The [patch](../vendor/patches/glib-0.18.5-variant-str-iter.patch) and
  [per-file provenance](../vendor/glib-0.18.5.provenance.json) are separate from the
  upstream tree. Do not run automatic format/fix tools on vendored files.

The offline verifier checks every file, rejects extras/symlinks/missing licenses,
reverses the two exact substitutions to recover the pristine file checksum, and
checks root Cargo selection plus a single local `glib 0.18.5` lock entry. Optional
archive verification reconstructs the entire original inventory from the pinned
published bytes. CI performs both, not just comparison to repo-controlled hashes.
This guards accidental drift; code review is still required because a malicious
PR can change the verifier and policy together.

```bash
python3 -B scripts/check-glib-backport.py
archive="$(mktemp -t ocean-glib.XXXXXXXX.crate)"
curl --fail --location --proto '=https' --proto-redir '=https' \
  https://static.crates.io/crates/glib/glib-0.18.5.crate --output "$archive"
python3 -B scripts/check-glib-backport.py --archive "$archive"
python3 -B -m unittest discover -s tests -p 'test_glib_backport.py' -v
```

The root patch applies when building Ocean as the workspace root. It does not
propagate to another project's workspace or to crates.io consumers. Root Cargo
publication is disabled until a registry-safe strategy is reviewed. Do not copy
this library into another workspace and assume the patch follows it. Keep locked
root builds and review any Cargo override changes.

## Validation and limits

The normal Rust test and a headless example share assertions for actual UTF-8,
empty and borrowed string values: next/nth/last, next_back/nth_back, mixed traversal,
exact size hints, exhaustion, singleton/empty arrays, overflow and wrong types.
The example runs under the application's **actual release profile** (size
optimization, LTO, panic abort), not the test harness's panic-unwind profile:

```bash
cargo test --locked --test glib_variant_iter
cargo run --locked --release --example glib_variant_iter_regression
cargo tree --locked -i glib@0.18.5
```

No GTK initialization, WebKit window, D-Bus/service control, provider or user
profile is used. Undefined behavior in the old code is compiler-dependent; test
success alone is not proof of safety. The exact reviewed upstream correction and
source/provenance checks are the primary evidence. Existing native CI separately
checks integration with Tauri/GTK/WebKit. Newer compilers can report cosmetic
lifetime warnings in the preserved old upstream code; do not rewrite that code to
silence them. Project Clippy checks remain strict and unchanged.

Static inspection found no named affected-iterator call in Ocean or the inspected
cached transitive Rust sources. This is **no observed call-site**, not an exhaustive
non-reachability argument across generated code, all builds or C/runtime paths, and
is not a basis for declaring the advisory irrelevant.

Scanners using only crate version may continue to flag 0.18.5, especially before
this PR is merged into their default-branch inventory. Do not automatically
suppress/dismiss them or claim a fixed upstream release. Record actual scanner
status and review this exact backport's evidence. Remove the override only when
all GTK/Tauri consumers select compatible fixed bindings and regression checks
pass. This does not address other advisories or complete the third-party binary
license inventory: that remains a separate pre-release gate, including the Rust
binding's preserved MIT terms and distinct system-library licenses.
