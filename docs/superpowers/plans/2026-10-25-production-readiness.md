# Ocean Production Readiness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox syntax for tracking.

**Goal:** Prepare reviewable Ocean stable and DSH next candidates for Linux x86_64/aarch64 with native localization and reversible installation.

**Architecture:** Preserve independent product feeds and frozen RC evidence. Fix product behavior on an isolated branch, then build architecture-specific candidates and exercise actual signed/package/native/update boundaries in disposable environments.

**Tech Stack:** Rust/Tauri/GTK3/WebKitGTK4.1, Node26.12.0, pnpm10.34.7, DSH, native Linux builders.

**Spec:** ../specs/2026-10-25-production-readiness.md

## Global Constraints

- Ocean stable; DSH next; x86_64 and aarch64 only. No ARM acceptance inferred from x86 tests.
- No source changes to working installations, immutable0.1.1, keys or existing releases.
- All build/test commands use /home/ryltsevmg/Projects/dsh-rc-prep.hLWKWv/exec-env.sh with production namespaces.
- No push, new release tags, signing or publication during preparation.
- Exact supported SDK versions; do not permit arbitrary future/prerelease variants or transfer consent across versions/profiles.

## Review Focus

- Shipped patched SDK version rejected by stock-only locale consent: task1 matrix regression.
- Regular executable .bin shim mistaken for SDK package: task2 anchor tests.
- Already running server silently misses dictionaries: task3 explicit activation and refusal tests.
- Private rollback confused with production-data rollback: task5 bounded scope and quiescence tests.
- Mutable native toolchain/vendor bytes or a missing ARM runtime closure: task4 per-architecture receipts.

### Task 1: Exact supported locale SDK contracts

Files: src/builtin_locale_consent.rs, locales/dsh/package.json, tests/builtin_locale_consent.rs, tests/builtin_locale_resources.rs.
Interfaces: preserve Scope/load/record and content-addressed resource cache. Allow the proven exact versions 0.2.0-rc.2, 0.2.0-rc.2.ocean.2, 0.2.1-rc.2; manifest peers match that explicit set.
- [x] Write consent and manifest regression tests covering the shipped pin, installed SDK, unknown-version refusal and version-bound grants.
- [x] Run tests and observe the existing stock-only rejection.
- [x] Implement the narrow compatibility set without editing live SDK/profile or recorded grants.
- [x] Run locale consent/resources/cache/planner/worker/revocation and client coordinator tests.

### Task 2: Installed SDK identity

Files: src/builtin_locale_planner.rs, tests/builtin_locale_planner.rs, tests/builtin_locale_worker.rs.
Interfaces: selected_sdk(&Runtime) returns the actual SDK manifest and exact version, including authenticated runtime regular .bin shim layout; foreign/ambiguous executables fail.
- [x] Reproduce the shipped regular-shim failure with a real package layout.
- [x] Implement narrowly verified anchor resolution; never execute a shim to guess ownership or use ambient lookup.
- [x] Test symlinks, foreign nearest manifests, missing targets and real relocated RC SDK metadata inspection.

### Task 3: User-visible localization lifecycle

Files: src/service.rs, src/localization_ui.rs, src/builtin_locale_planner.rs and matching lifecycle/native tests; exact UI design follows existing ownership rules.
Interfaces: explicit native action prepares an owned restart or reports attached/custom ownership limits; it never silently mutates an existing server. New installs use locale plan before launch.
- [ ] Add an end-to-end regression proving the first normal launch actually advertises Russian.
- [ ] Implement the smallest explicit activation workflow retaining consent/ownership/pending gates.
- [ ] Verify Chinese/English plus all seven added languages, selection and completed durable write before reload; removal/disposal and recovery.
- [ ] Cover stock/patched/0.2.1 SDK compositions and duplicate manual packs without broad compatibility exemptions.

### Task 4: Native stable shell and next runtime candidates

Files: Cargo.toml, Cargo.lock, CHANGELOG.md, runtime-manifest.json only if justified, release documentation and external artifact/evidence manifests.
Interfaces: production acceptance consumes exact native archive hashes, source commits, native receipts and license/source materials for each architecture.
- [ ] Freeze final source and shell0.1.2 after behavior tests; confirm existing channel/tag history.
- [ ] Resolve builder/toolchain availability and native ARM dependency closure before build; preserve working Omarchy.
- [ ] Build tar/deb and complete runtime per architecture with the pinned audited toolchain and notices.
- [ ] Run package allowlist/native/ABI/GUI and independent artifact verification, not just --version.

### Task 5: Upgrade and recovery acceptance

Files: existing installed/native/managed-runtime/plugin tests plus isolated evidence helpers under the production work root.
Interfaces: immutable generations, exact service identity, authenticated HTTP and scoped data snapshots.
- [ ] Test fresh install, legacy adoption where supported, previous release update, failed activation and rollback on each architecture.
- [ ] Validate fixed tool-management against the actual profile dependency set, including Codex; refuse unsupported plugins without auto-grants.
- [ ] Exercise restart/re-authentication, explicit dictionary activation, post-reload language and preservation of unrelated settings.
- [ ] Verify independently trusted keys/floors using disposable settings and honest signed-runtime baseline limits.

### Task 6: Final review and handoff

Files: production report, artifact manifest, source freeze, checksums, review findings and exact publisher proposal.
- [ ] Run required fmt/clippy/locked tests, JS/client and packaging/native gates appropriate to final diff.
- [ ] Perform independent whole-branch review and resolve Important/Critical findings.
- [ ] Record complete passed/failed/not-run matrix and outstanding decisions with evidence.
- [ ] Prepare exact stable-shell/next-runtime version, sequence, source/artifact proposal for final authorized release step. Do not publish as part of preparation.
