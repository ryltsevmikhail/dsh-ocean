# Window lifecycle slice

Scope authorized by user: next slice after 0.1.4, including explicit Quit.
Work only in the independent window-quit checkout, codex/window-quit.
No release, signing, push, host installation, working service/profile/key changes.
Versions remain unchanged until a separate release preparation.

1. Expose localized native Ocean > Quit with Ctrl+Q without relying on tray.
   Use existing ExitRequested cancellation/commit deferral; do not expand web IPC.
2. Restore setup controls from native installation/update state after reload.
   Cancel must work without a page-local start; duplicate prepare/install refused.
3. Own outcome and prepare continuation in native task, not renderer observer.
   Preserve bootstrap guard and avoid prepare after quit was requested.
4. Correct backup CLI help and localized result text to describe explicit offline
   restore, retained roots and no schema migration.
5. Run meaningful reload regressions, ownership test, Rust/JS/locale checks;
   independent whole-branch review before reporting implementation complete.

Acceptance bounds: source slice only. No claim of new packaged or installed release,
ARM64 native execution or actual GUI interaction unless separately evidenced.

Ledger:
- Quit compiled; existing exit gate tests 3 PASS.
- Two real setup reload regression tests observed RED, then both GREEN (11 JS total).
- Native task ownership test checks dropped observer, retained exclusive gate,
  terminal state and gate release; PASS.
- Ruling: reuse native window menu and existing translated tray label. This avoids
  exposing a new quit command to server content; cost: user opens Ocean menu.
- Ruling: detached native continuation covers completion and service preparation;
  quitting skips preparation. Existing runtime lifecycle still prevents second server.
- Final verification: cargo test --locked --offline --tests: 616 PASS, 30
  explicitly ignored across 78 binaries. Node setup polling: 11 PASS. Python
  native locale catalog checks: 4 PASS. cargo check and diff --check PASS.
- Additional actual WebKit mapped-window reload test under private bus + Xvfb:
  PASS. IPC backend is a fixture; this is not a live runtime install acceptance.
- Independent review Sagan 01a17853-24dc-7bd0-b4cd-ded52e629533 accepted product
  diff with no Important/Critical findings; no product changes after review.
  Review limit: source only, native menu/accelerator not exercised interactively.
- Deferred verification: ARM64 build/execution, packaged install/upgrade/rollback,
  actual native menu activation and live first-install reload. No new release
  identity or publication was created; this is a checked source slice.
