# Signed First Installation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Ocean0.1.3 installs the accepted custom DSH from signed prebuilt bytes on a clean Linux host.

**Architecture:** Separate first-install module reuses verifier/download/import/native health, with a narrow durable journal and no existing-runtime update authority. GUI owns one cancellable native worker and preserves existing service/locale flow.

**Tech Stack:** Existing Rust1.90/Tauri/Tokio, Ed25519 verifier, native Ubuntu botharchitectures.

**Spec:** 2026-10-26-signed-first-install-design.md

- [x] Task1: runtime_install.rs + meaningful failing regression/negative/recovery tests, then minimal implementation. Pin exact public runtime contract; cancellation, retained import validation, nooverwrite selection, journal/floor consistency. No unrelated source edits.
- [x] Task2: state.rs/commands.rs own Control and worker lifecycle; bootstrap.rs startup journal guard; runtime_maintenance.rs failclosed guard; test first-install cancellation/quit/latecommit/cache refresh and malformed journal guard. Export module in lib.rs.
- [x] Task3: bump Ocean0.1.3 consistently, installation docs/expiry limitations, fmt/clippy/relevant regression + full required suites. Independent wholepatch review before rebuild. No publication until new identity/locks/proofs valid.
- [ ] Task4: frozen newsource commit/export, native Ubuntu rebuild/tar/deb botharchitectures, exactpackage inventory, real freshinstall GUI/native + update/rollback, unchangedruntime signed acceptance botharchitectures. Preserve0.1.2.
- [ ] Task5: new prebuilt shell identity/provenance/metadata signing, exactsigned nativeacceptance, final independent actualproof review; runtime publish/publicvalidate before shell promotion/publicdownloads; final report.

Review Focus: trust/policy confusion, unsafe paths, locking, cancel/quit/commit races, crash/retry selector/floor consistency, external runtime preservation, tamper reuse, updater trust unchanged, actualzero-runtime native launch botharchitectures.

## Verification ledger

- Python packaging/helper suite179/179 PASS (logs/ocean013-python-tests.log).
- Task1 initial implementation:17newtests, observedRED→GREEN,241librarytestsPASS; authorPeirce. Not native acceptance.
- Task2 cancellation regression: removing actual Control.cancel hook causes both meaningful state tests to fail; restored hook passes (logs/ocean013-cancel-regression-{red,green}.log).
- Finalreview Jason:4Important, noCritical. Onefixpass active; no secondcode-reviewloop. Quitfix1 reproduced via state.updates exit gate failing toWait, then registers installer withrealRegistry; GREEN. Worker gate dropsLAST after allcleanup; actualGUIQuit phases remain required native acceptance.
- Final: minor (deferred): setup-page reload leaves page-local cancel guard stale despite native state. User can cancel through originalpage; nativeQuit ownership fixed separately.
- Final: minor (deferred): outcome logging/automaticservice preparation still await IPCfuture; a reloadedpage can explicitlyprepare the completedinstallation.
- Final: Ruling: distinct immutablebootstrap uses compiled exactsignedmetadata/artifact identity and existingkey, allowing those fixedreviewedbytes beyond updaterexpiry. Ordinary updaterfreshness/anti-equivocation unchanged. Cost ifwrong: this fixedOcean version can install a retiredfixedruntime whileits exactassets remain available; remediation requiresnewOceanrelease.
- Declined acceptance/provenance points remain required laterstages, not waived: nativeGUIzero-runtime botharchitectures, finalpackagebytes/sourcebindings and publicdownloads.

- Final: fixed interrupted-import retry — retry_removes_only_private_import_scratch_and_preserves_foreign_entries RED→GREEN; additional empty-unmarked/corrupt/foreign/FIFO scratch cases pass.
- Final: fixed retained-tree authenticity — retained_payload_inventory_and_receipt_cannot_be_rewritten_together RED→GREEN; expected inventory now derived from fresh authenticated archive and retained inside Prepared across later checks.
- Final: fixed fixed-installer expiration — immutable_bootstrap_is_exact_signed_identity_and_not_mutable_feed_policy RED→GREEN; sealed exactbootstrap proof, separate nativehealth entry; normal signedfeed expiry regression remains passing.
- Final: fixed Quit cleanup/commit deferral — quit_waits_for_native_installer_cleanup_and_cannot_cancel_commit RED→GREEN; shares actualnativeRegistry and worker operationdropsLAST.
- Full Rustsuite615passed/0failed/30ignored (logs/ocean013-install-fixes-full-final.log). All-targetClippy PASS (logs/ocean013-clippy-all.log), cargo fmt --check PASS after main formatting; license inventory --check PASS. Rootmetadata hashes regenerated for0.1.3 only, no dependency/license changes.
- Initial extra setup-contract ignored invocation was a NO-OP without DSH_OCEAN_TEST_DIALOGS=1; do not count as GUI proof. Corrected real privateXvfb invocation pending logs/ocean013-setup-webview-real.log. Actual packagedzero-runtimeGUI remains separate pending proof.

- Actual mapped private GTK/WebKit setup contract PASS (logs/ocean013-setup-webview-mapped.log): install command and cancellation controls exercised. Earlier unmapped attempt failed because fixture requires DSH_OCEAN_TEST_MAP_WEBVIEW=1; unchanged product/UI. This is an IPC-stub page contract, NOT packagedzero-runtime native acceptance.
- Source-fix gate complete:4Important fixed with observed meaningfulRED→GREEN, full615/0/30, all-targetClippy, fmt,179Python, exactinventory and mappedsetupcontract. No claimed artifact rebuild/nativefirstinstall/publicacceptance yet. Frozennewsource/build next.
