# Releasing

This describes the **local** release checklist for DSH Ocean. It is not a
published trust root: nothing here signs, attests or auto-publishes anything.

1. The declared `rust-version` in `Cargo.toml` is checked by a separate CI job
   (`cargo check --locked --all-targets` on exactly that toolchain), so a
   stable-only language feature cannot silently raise the MSRV.
2. `shellcheck -x -S warning scripts/*.sh packaging/*.sh` (also enforced in CI).
3. `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`,
   `cargo test --locked`, the Node test files and
   `python3 -B -m unittest discover -s tests -p 'test_package_*.py' -v` pass.
   CI disables incremental compilation and dev/test debug symbols to leave room
   for GTK/WebKit integration binaries and the release build; assertions remain
   enabled. The disk-usage step reports remaining space even after a test failure.
   Offline installer fixtures control `df` output, including checks immediately
   below and at the production free-space threshold; they do not bypass that guard.
4. Every packaging smoke passes (`tests/test_package_tar.py --real-smoke`,
   `--real-smoke` for Debian, `--real-appdir-smoke`, and
   `tests/test_package_install.py --real-smoke`).
5. `Cargo.toml` version is bumped; `CHANGELOG.md` has a matching section
   (CI checks that `Cargo.toml`'s version appears in the changelog).
6. The launch smoke passes: `bash scripts/test-native.sh xvfb-run -a bash
   scripts/smoke-app-launch.sh target/debug/dsh-ocean` keeps the real binary alive
   for its timeout without a panic **and** that a `DSH Ocean` window is mapped
   (`xwininfo`); without the window the smoke fails instead of passing on a
   headless-but-useless process. It also launches a second copy and requires it to
   hand over and exit promptly, leaving exactly one window and one live instance. It is wired into CI as its own step and uses
   private XDG directories, an unroutable origin and software rendering.
   It also sends real X11 `WM_DELETE_WINDOW` messages: default close must hide
   the still-live window, another launch must restore it even while an idle
   activation-socket client is connected, and a subsequent close
   with `closeToTray: false` must exit cleanly. This uses the normal close handler,
   not window destruction or a kill signal; configuration writes stay within
   the private harness. The X11 helper refuses a non-fixture environment.
7. Runtime maintenance was re-checked: `--runtime-maintenance rollback` feeds the
   embedded health helper and preserves differing installed units. A selector
   rollback does not retarget an existing unit: review `--print-unit`, explicitly
   replace the unit, then reload/restart in a maintenance window. `--gc` never
   collects the generation named in the installed unit's ExecStart.
8. Native acceptance ran on an isolated display/D-Bus: GTK dialogs, WebKit
   downloads and the backup verification/history windows.
9. Artifacts are built with the exact commit, `pkg-config`-compatible system
   WebKitGTK 4.1/GTK3, and a clean `dist/` (`SOURCE_DATE_EPOCH` may be set for
   reproducible tar/deb archives).
10. Release notes list user-visible changes and explicitly state the limits that
   still apply: system WebKit dependency, no production signed feed (the opt-in
   local anti-rollback store is not publisher authentication), no data/schema
   rollback, no portable-ABI guarantee, and that AppImage tool
   trust is an operator-provided digest.

Tagging, signing, publishing and update-feed configuration are **not** covered by
this document. Do not describe a build as verified, signed or reproducible unless
independent evidence exists.

## Past internal 0.1.1 local preparation — 2026-10-20

The following records private-development validation, not public CI or published
release evidence. Check current public CI against the exact published head and
its final conclusion. Host-specific limitations below describe
the historical validation environment, not every contributor's machine.

- The intermittent pairing-plan fixture loss has a reproducible ownership race:
  same-PID parallel tests can sample equal wall-clock nanoseconds, and the old
  `create_dir_all` accepted a shared fixture root. A peer's Drop then deleted the
  survivor's config. Natural equal clock samples were observed locally; injecting
  the same sample into the old fixture reproduced ENOENT after a successful CLI
  call. This matches the historical failure but is not forensic proof of that
  particular unlogged event.
- Fixtures now append an atomic per-process serial and exclusively reserve the
  root before populating it. A fixed-clock concurrent regression verifies distinct
  roots and surviving config after peer cleanup. All four pairing-plan tests
  passed 50 consecutive parallel runs; no sleeps or acceptance retries were added.
- Real native aarch64 tar and Debian artifacts passed allowlist, metadata,
  checksums and isolated headless `--version` smoke. Independent `ar` inspection
  passed. `dpkg-deb` is absent on this host, so its independent reader and actual
  Debian installation remain untested locally.
- A real trusted-local tar passed preflight, fresh installation, GLib desktop Exec
  decoding, private ledger checks, `--version` and uninstall under a disposable
  HOME/XDG/prefix. Unrelated files survived; no real service/cache helper ran.
- A real AppDir/AppRun passed headless version smoke in a spaced private path.
  No external AppImage tool was selected/executed and no genuine AppImage image
  was built. No production key, tool digest or source-archive trust was created.
- `cargo fmt --check`, strict all-target locked clippy and the full headless
  `cargo test --locked` passed. The version bump exposed a reset-state test that
  incorrectly assumed 0.1.0 was still acceptable to the current binary; its
  post-reset manifest now uses the current package version. Runtime downgrade
  protection was not weakened. Ignored native/network tests remain ignored.
- Pure JavaScript tests: 24 passed. Python packaging/source-install/Make tests:
  42 passed. Source shell syntax and diff whitespace checks passed.
- Past internal manual acceptance covered core desktop behavior on a development
  binary still reporting 0.1.0. This is not a fresh GTK/Xvfb run or a desktop launch of the
  versioned 0.1.1 packages. Xvfb and shellcheck are absent locally; ignored native
  tests, real-service/network updates and the distro matrix remain separate gates.
- The working desktop client and DSH service were not replaced or restarted during
  0.1.1 preparation. Artifacts are local unsigned builds, not published releases.
