# Scoped native release review: portable Linux shell tar

This review concerns **Rust 1.90.0, native Ubuntu 24.04 x86_64 and aarch64 shell
tars**. It is not clearance for arbitrary compiler builds, older Linux ABIs,
AppImages, Debian/Arch publication or rebundled Node/DSH runtimes. No owner
attestation is issued by this document or the evidence scripts.

## Concrete defect corrected

The previous portable payload retained Cargo notices and MPL sources, but not the
standard-library/compiler-runtime notices embedded in the executable. A passing
source inventory or a temporary CI copyright report did not fix that omission.
The explicit packaging/portable/source-install allowlists now retain the pinned
`docs/toolchain/rust-1.90.0/` profile with the installed notice payload. Modified
files remain ownership-ledger protected on uninstall.

The exact compiler source commit is
`1159e78c4747b02ef996e55082b704c09b970588`. Distribution inputs are the dated
[official Rust 1.90.0 manifest](https://static.rust-lang.org/dist/channel-rust-1.90.0.toml)
(2025-09-18); the profile index records exact archive/member/source hashes.
`check-release-toolchain.py` independently checks retained bytes and, on each
release runner, the actual compiler commit/native target and installed notices.
It does not relabel the existing Rust 1.99.1 research profile.

### Why retain the full compiler-wide document too?

The exact distribution `COPYRIGHT-library.html` is 360,726 bytes, but its generated
list does not name compiler-builtins/libm or some distributed std dependencies
(addr2line/adler2/object/miniz_oxide). It must not be called complete merely because
it is nonempty. We retain the **entire unmodified** 12,368,099-byte distribution
`COPYRIGHT.html` and all offered license texts conservatively, plus the exact
compiler-builtins/libm license files. Nothing is truncated or split; the generic
artifact collector's 4 MiB notice-input bound is unchanged. These explicit package
inputs fit the existing 16 MiB per-document packaging bound.

The complete document contains notices for compiler components not necessarily
linked into Ocean. Their inclusion does **not** say Ocean contains all those
components or that all those licenses apply to the whole executable. Scope and
artifact-bound observations remain separate from redistribution approval.

The unchanged official `rust-src-1.90.0.tar.xz` retains std/compiler-builtins/libm
per-file notices (including Sun math and other preserved notices). It is not all
vendored dependencies or compiler-rt source. The separately retained compiler-rt
credits come from Rust's exact LLVM gitlink
`e8a2ffcf322f45b8dce82c65ab27a3e2430a6b51`, not a moving `main` reference.

## Review treatment / decision table

| Component | Retained material and scoped treatment |
| --- | --- |
| Ocean source | Existing MIT terms and project notices. No whole-ELF MIT claim. |
| Cargo dependency union | Existing full offered texts/notices retained. OR/legacy slash declarations are not flattened; both offered texts remain. AND obligations remain cumulative. |
| Cargo Unicode / exceptions | Existing Unicode-3.0 and target-lexicon LLVM-exception terms remain; generic MIT/Apache labels do not replace them. |
| Five MPL crates | Existing exact covered-source `.crate` archives, hashes and recipient access remain. No source modifications hidden behind old archives. |
| Rust std | Exact 1.90 distribution full/library copyright and all distribution license files, root MIT/Apache, source archive and provenance. Other compiler versions are outside this profile. |
| Compiler-builtins/libm | Exact source licenses, including MIT **AND** Apache-2.0 WITH LLVM-exception and applicable additional offered terms; preserved per-file source notices and immutable compiler-rt credits. |
| GNU unwinding / compiler runtime | Record the actual linker inputs and system package origins. Do not infer LLVM libunwind membership from rustc's LLVM version; do not drop GCC runtime exception requirements where applicable. Relevant distribution license texts are retained; selected static system inputs still require review of their exact origins/terms. |
| GTK/WebKit/libsoup/appindicator and other host shared libraries | Portable tar copies no shared-library tree. Native evidence records loader-resolved system libraries and dpkg origins, explicitly marked **not redistributed by tar**. Later plugins/dlopen/subprocesses are not asserted universally inventoried. |
| Original Ice artwork / embedded UI/scripts | Existing project `NOTICE` and branding provenance assertions apply; retain/hash supplied assets. Automated byte checks do not prove authorship or contributor authority; owner confirms the existing declaration. No font files are deliberately bundled. |

Retaining offered terms is not a new upstream license election or a claim that
all expression branches apply to all linked bytes. Review the actual component,
applicable exception and notice rather than the inventory's syntactic
`manual_review` flag.

## Exact native evidence path

For a shell candidate the native release workflow:

1. Pins rustc 1.90.0 and checks its full commit, host target and installed public
   notice bytes against the reviewed profile.
2. Uses a separate fresh Cargo target directory and explicit `/usr/bin/gcc`
   driver with GNU BFD (`-fuse-ld=bfd`), recording both versions. It captures a map
   from that actual release build; no release/admission guard is relaxed. The GNU
   map OUTPUT must identify a Cargo deps executable with the exact reported ELF
   digest, not merely contain an Ocean substring.
3. Builds the final tar normally. The collector rehashes **every shipped file**
   against the checkout and the archive's executable against that exact Cargo ELF.
   Stale/rebuilt ELF, omitted/changed notice bytes and extra/duplicate paths fail.
4. Runs the existing isolated X11 real-release-ELF launch/single-instance/close/
   restore smoke, not only `--version`, on both native architectures.
5. Records ELF interpreter/DT_NEEDED, ABI symbol requirements, map-referenced
   archive/object hashes with sanitized full path/member identity, system loader
   dependencies and dpkg package/source versions. The reference list includes
   LOAD/discarded entries: it is **not** selected-code membership. Missing inputs
   are recorded, not silently omitted. GNU/lld formats are distinguished in tests;
   the release collector requires its explicit GNU pipeline and leaves section
   membership/notice applicability to map review. Loader inspection runs only on
   the trusted locally built executable.
6. Retains **public report/map only**, including dry runs. No binary, profile,
   credentials or signing key is in the report artifact. Maps replace workspace,
   sysroot, target and build-home prefixes. Original and published-map hashes are
   recorded separately. A future draft binds report source/archive hashes before
   attaching architecture-qualified evidence alongside the binary tars.

This is build-associated evidence from the controlled workflow, not a universal
linker SBOM or external build attestation. Hosted evidence has limited retention;
archive matching reports/notices with the authorized final release.

## Before the owner attests

Read both **actual final native** reports/maps and shipped notice payloads. Resolve
any unowned/unresolved static origin or unexpected copied library, inspect the
remaining GCC/runtime/asset decisions above, and retain matching evidence. Confirm
the supported ABI claim is Ubuntu 24.04/native GTK3/WebKitGTK4.1, not all Linux.
The explicit `OCEAN_RELEASE_CLEARANCE` and protected owner approval remain separate
from green CI, key creation and a general request to publish. Production signing,
tag/source binding and final public preview publication still require the existing
gates. No tool here sets that clearance or accesses production keys.
