# Releasing

This describes the **local** release checklist and the owner-operated manual
signed-candidate workflow. It embeds no production trust root and does not grant
release clearance. The workflow never publishes a public release automatically.

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

Do not describe a build as verified, signed or reproducible unless independent
evidence exists. Public trust provisioning/rotation, compatibility acceptance and
final public release/feed activation remain explicit owner actions.

## Manual signed release candidates

[The release workflow](../.github/workflows/release.yml) runs **only** through
`workflow_dispatch`. Choose `shell` or `runtime`, independent `stable`/`next`
channel, exact repository version and an owner-assigned positive monotonic
product/channel sequence. Prereleases are refused on `stable` at preflight; the
current pinned prerelease runtime must use `next`. No production sequence/key/
fingerprint is invented.

**Dry-run defaults to true.** Each native x86_64/ARM64 runner builds its own
artifact and the independent Rust verifier. Hosted ARM runner availability/billing
is an owner prerequisite; an unavailable native runner is not permission to skip
native health or label a cross-build as tested. Runtime builds use the repository's
Node archive checksum, DSH/pnpm pins and lifecycle/native compilation **on the
publisher**, never on an installed client. Complete archives retain dependency
bytes/notices. Disposable validation keys authorize only isolated CI checks,
including runtime native bindings/version/authenticated health and shell apply
into a new private prefix. Dry-run reads no production signing secrets, uploads
no binaries and creates no release/tag. It is not publisher authentication or
production artifact/laptop acceptance.

Non-dry-run is additionally restricted to manual execution on `main`. It uploads
short-lived candidate artifacts, then enters the product-specific protected
`release-shell` or `release-runtime` environment. **The owner must configure both
environments with required reviewers and main-only deployment rules** before use;
creating this workflow does not configure GitHub protection. Checkout credentials
are not persisted into supplier builds. Producer/native checks occur in separate
build jobs before production keys are made available.

Configure only the matching protected environment's inputs:

| Product | Secret PEM | Public variables |
| --- | --- | --- |
| shell | `OCEAN_SHELL_SIGNING_KEY_PEM` | `OCEAN_SHELL_KEY_ID`, `OCEAN_SHELL_PUBLIC_KEY_SHA256` |
| runtime | `OCEAN_RUNTIME_SIGNING_KEY_PEM` | `OCEAN_RUNTIME_KEY_ID`, `OCEAN_RUNTIME_PUBLIC_KEY_SHA256` |

The fingerprint is SHA-256 of the raw 32-byte Ed25519 public key, **confirmed
independently**, not copied from the untrusted release being checked. Never put
private PEMs in repository files, artifacts, logs, release notes or an assistant
request. The signing helper uses private temporary files, verifies the independent
public fingerprint, exports only public signature/key files and removes its key
scratch. Missing approval/key/fingerprint or a mismatch fails closed. Separate
product steps avoid a missing runtime secret falling back to a shell key.

Before keys are accessed, the environment variable `OCEAN_RELEASE_CLEARANCE`
(GitHub environment **variable**, not a secret) must equal
`owner-reviewed-native-and-license-clearance`, and the canonical tag must already
exist (`vVERSION` for shell; `runtime-CHANNEL-vVERSION-rSEQUENCE` for runtime)
and resolve to the **exact commit built by this run**. A mismatched/missing source
binding refuses key access. Protect release tags against later movement.
This is a deliberate **owner attestation**, not an automated legal/native ABI
proof. Inventory/source checks do not close compiled compiler/std, native/dlopen,
genuine package/assets or migration acceptance gates. Do not set clearance merely
because tests passed; review the [native/toolchain audit](TOOLCHAIN_NATIVE_AUDIT.md).

[Metadata assembly](../scripts/release-artifacts.py) requires both native targets,
canonical URLs and actual archive size/SHA-256; it rehashes the adjacent files
before preparing the manifest. Single-target manifests require the explicit
validation-only flag and are not used by the draft job. Runtime components and
original Node archive hashes must match the repository pin. Manifests have a
seven-day default lifetime (bounded by the client's thirty-day maximum).
[Protected signing](../scripts/sign-publisher-release.mjs) selects the explicit
product domain; [the low-level signer](../scripts/sign-release.mjs) still requires
independent Rust policy validation. Final production signatures and both exact
artifacts are rechecked by the Rust client before a draft can be created.

The last step uses `gh release create --verify-tag --draft`: **draft only**, no
automatic tag creation or public publication. The owner must inspect genuine
artifacts, signatures/notices, native compatibility and acceptance, then separately
publish/configure authenticated feed endpoints. A public key attached to a release
is not itself an authenticated trust root. This development work does not invoke
the workflow or access production keys.

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
