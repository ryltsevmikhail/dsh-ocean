# Artifact-bound toolchain and native evidence

**Status: observational evidence, not release clearance or build attestation.**
This complements the Cargo source inventory; it does not replace it or certify
all statically linked, indirect native, plugin/dlopen or AppImage components.
No binary release, live installation or runtime/profile operation is performed.

## Why another evidence layer is necessary

The application's Cargo.lock does not fully inventory Rust std, compiler-builtins,
unwinding/backtrace or compiler runtime. A current `rustc -vV` does not prove which
compiler built an arbitrary or cached ELF. A build ID or `.comment` is not that
proof either. Likewise, `DT_NEEDED` is a direct name list, not a loader resolution,
a package provenance record or a complete native license inventory.

A concrete local research case (2026-10-21) demonstrated the danger of assuming
notice completeness:

- The observed Arch compiler was Rust 1.99.1, package `rust 1:1.99.1-1`, source
  commit `58cf8f9c35b355199727627d07ae34a4e2846d67`, host aarch64 Linux GNU,
  LLVM 23.1.3. This is a research observation, not a pinned project toolchain.
- Its installed `COPYRIGHT-library.html.rustc` was only 4,021 bytes and had an
  **empty out-of-tree dependency section**. The full installed copyright HTML
  also did not supply a complete dependency list/full applicable texts.
  Installed notice presence must not automatically clear the std gate.
- The exact-commit compiler-builtins 0.1.160 manifest declares
  `MIT AND Apache-2.0 WITH LLVM-exception AND (MIT OR Apache-2.0)`.
  Reducing this to "Rust is MIT/Apache" loses required terms/exceptions.
- That source tree's GNU/Linux unwind implementation has conditional gcc_s and
  alternative LLVM/static paths. An LLVM version reported by rustc is not proof
  that LLVM libunwind or all compiler-rt code was embedded in this application.

The source [Rust copyright guide](https://github.com/rust-lang/rust/blob/58cf8f9c35b355199727627d07ae34a4e2846d67/COPYRIGHT)
explains how generated distribution copyright HTML is built. The exact-commit
[compiler-builtins manifest](https://github.com/rust-lang/rust/blob/58cf8f9c35b355199727627d07ae34a4e2846d67/library/compiler-builtins/compiler-builtins/Cargo.toml),
[license terms](https://github.com/rust-lang/rust/blob/58cf8f9c35b355199727627d07ae34a4e2846d67/library/compiler-builtins/LICENSE.txt)
and [unwind source](https://github.com/rust-lang/rust/blob/58cf8f9c35b355199727627d07ae34a4e2846d67/library/unwind/src/lib.rs)
are evidence for those source statements, not proof of linked membership.

## Pinned source profile

The development checkout retains an eight-file source profile under
`docs/toolchain/source-evidence/rust-58cf8f9c35b3/`, with the Rust MIT/Apache texts,
compiler-builtins/libm notices, relevant manifests/unwind source and upstream
copyright guide. Its index records immutable URLs, Git blob IDs and SHA-256.
The checker pins that reviewed index and rejects changed/extra/linked evidence:

```sh
python3 -B scripts/check-toolchain-source-evidence.py
```

These are **source-profile supplements**, not a universal standard-library
binary notice set. They do not cover unreviewed distro patches, actual features,
all external std dependencies, all per-file notices or arbitrary compiler releases.
A different compiler commit requires a separate reviewed profile; never label this
profile as its source merely because the release string looks similar.

## Inspect one exact ELF without running it

From a trusted source checkout after building the intended artifact:

```sh
python3 -B scripts/artifact-license-evidence.py \
  --binary target/release/dsh-ocean \
  --output-dir /tmp/ocean-artifact-evidence-NEW
```

The output directory must be new; no old report is overwritten. The report binds
SHA-256, byte size and supported ELF identity to that exact byte sequence. The
collector parses program headers/dynamic strings rather than executing the client,
its interpreter, `ldd` or arbitrary libraries. Unsupported/malformed input fails;
an absent dynamic section must not be confused with failed parsing.

It records direct dependency names, standard interpreter information and redacted
search-path classifications, not a filesystem walk or guessed loader closure.
Compiler fields and root lock/source-inventory hashes are **observed context**;
they do not authenticate original compilation, feature flags, link membership or
an independently reproducible build. Wrapper/override presence is recorded without
publishing raw environment values, sysroot/home paths or command stderr. Compiler
execution is for a trusted installed tool/explicit trusted `--rustc`, not an
arbitrary-wrapper sandbox: I/O/leader limits and timeout cleanup do not establish
containment of deliberately detached or normally exiting wrapper descendants.
Cargo-style executable hardlinks are accepted read-only with descriptor/stamp
checks; notice/input documents remain single-link and no input is rewritten.

Explicit `--notice LABEL=FILE` arguments can retain bounded public license/copyright
candidate files with hashes. They must be reviewed public terms, never profiles,
credentials, arbitrary user files or entire sysroot/package trees. The collector
requires owned single-link notice files outside hidden profile/cache directories;
review and stage specific public system/rustup copyright files in a new private
working directory first, without treating that copy as coverage or package-origin
proof. At most 16 candidates / 16 MiB total are retained (4 MiB per candidate).
Copying a file
called COPYRIGHT cannot prove that its coverage is complete. An empty, absent or
incomplete distribution dependency section remains unresolved.

CI captures this report after the fresh release-build step and retains **reports
and notice text only**, not the ELF, as a short-lived Actions artifact. This is
review evidence, not a public binary release/signature or an installed license
payload. The 14-day CI retention is only for review: archive matching evidence
and required source/notice payloads durably with any future authorized release.
Package helpers' existing Cargo notice/covered-source retention is not
silently treated as complete runtime/toolchain coverage.

### Confirmed CI capture refusal and bounded selection

Diagnostic [run 39603165011](https://github.com/ryltsevmikhail/dsh-ocean/actions/runs/39603165011)
on head `80838ee96dc3332895240074c8bf1b1f1685a032` measured the installed Rust
1.99.1 library copyright candidate at 1,499,465 bytes and compiler-wide
`COPYRIGHT.html` at 14,423,589 bytes. Capture refused the latter with
`input size exceeds limit`; this was not an ELF architecture or ownership failure.

CI now deliberately selects only the two known library copyright locations,
plus matching source-profile supplements. It does not select compiler-wide
`COPYRIGHT.html` or `COPYRIGHT.html.rustc`. The retained
`docs/toolchain/NOTICE-ci-scope.txt` explicitly records this gap and the historical
measurement; it is scope context, not an upstream license or a claim about the
current toolchain's bytes. No truncation, splitting or larger limits are used.
Selected library notices still fail closed above 4 MiB. Omitted applicable
compiler-wide material needs separate review/retention before redistribution;
a green capture does not clear compiler/runtime/std coverage.

## Native origin and redistribution decisions still required

For the actual target/release, record which bytes are copied, which libraries are
system-provided, and how the executable was linked. The current helper tar/deb/AppDir
layouts intentionally copy one client ELF and their explicit documentation/assets;
they do not themselves run a native-library bundler. That boundary is not proof
about a genuine AppImage tool/runtime output or an alternative bundler.

For every relevant native component, review:

1. Exact final artifact hash and actual direct/indirect/dlopen/static membership.
   Kernel/GTK/WebKit subprocesses and optional plugins need separate investigation.
2. Package/distribution/source origin, versions, modifications and applicable terms.
   `pacman -Qo`/`dpkg-query -S` claims are candidate package database observations,
   not actual loader selection or authenticated build origin. Do not follow user
   LD_LIBRARY_PATH/cwd/home paths to copy "licenses" automatically.
3. Required copyright/NOTICE/full terms, license elections and exceptions. The GCC
   Runtime Library Exception has eligibility conditions; an unrelated AIX header
   exception in Rust's copyright metadata does not establish Linux libgcc coverage.
4. For copied LGPL/GPL/native code, applicable source, relinking/replacement and
   notice obligations. Dynamic linkage alone is not a complete legal determination.
5. Real distro/ABI/package-name matrix, genuine AppImage runtime/source/download
   provenance and exact extracted file list. A trusted tool digest does not pin
   its downloads, and AppDir/stub tests do not establish genuine-image compliance.

Current reports always leave these release gates open. To clear them later, retain
matching compiler/distribution manifests, source/patch/feature/link evidence, actual
component notice payloads and a deliberate maintainer review for that exact artifact.
Do not publish based solely on a green collector test or a nonempty notice folder.
Other raw-ELF/updater routes, later Node/DSH/pnpm downloads and art/contributor
provenance remain bounded by the [overall binary audit](BINARY_LICENSE_AUDIT.md).
