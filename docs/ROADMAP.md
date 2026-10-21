# DSH Ocean — roadmap to 1.0

Status: planning. The 0.1 client works: it attaches to a local DSH server,
survives service restarts, has a tray, diagnostics, bootstrap and packages.
This roadmap closes the six gaps identified in the comparison with
`dsh-tauri`, without forking the DSH web UI or turning the shell into a second
harness.

## Principles

- The official `@deepseek-ai/dsh` and web frontend stay upstream. DSH Ocean is
  a native shell and desktop layer, not a fork.
- The shared `~/.dsh` profile and sessions stay the source of truth.
- Loopback is the default. Remote/mobile access is explicit, temporary and
  revocable.
- The remote webview gets no `fs`, `shell`, `http` or updater capability plugins.
- Every milestone has an isolated test path. Never stop the live server from
  inside a running DSH chat.
- Linux first. Windows/macOS are separate follow-up work, not part of 1.0.

## Current baseline

Validation described below records past private development unless explicitly
stated otherwise. It is not evidence of a run on the first public snapshot;
verify current public CI against its exact head. Publication starts a new root,
not the project's development history.

| Area | State |
|---|---|
| Server attach/systemd/child | Attach and systemd switch live-tested; offline child fixtures cover readiness/timeout/early exit/kill. An opt-in real-runtime test now validates the production child-start helper with private HOME/XDG/env/cwd, temporary ports, 401/303 auth, credential rotation, process restart and confirmed reaping; fixture-only subreaper watchdog coverage runs by default. The controlled-child Xvfb GUI smoke passed in Ubuntu 24.04 x86_64 CI on 2026-10-20 for fallback/recovery/window reload and close cleanup. Real DSH UI and installed-client window acceptance remain pending |
| Token auth from journald | Working, validated with 303 exchange |
| First-run bootstrap | Whole-runtime generations with atomic selection, isolated authenticated health checks and selector rollback; legacy migration, npm lifecycle approval, `--check` and `--rollback`. Cooperative cancellation (process-group `TERM`, bounded `KILL` escalation, one-shot gate, honest `Committed`/`Cancelled`/`Failed`) is implemented and tested offline against the real script; stale scratch is swept on every non-`--check` run and `--gc` prunes unreferenced generations while keeping the selected one plus one rollback hop. `dsh-ocean --doctor` additionally prints read-only runtime counters
(selected/previous/known/unreferenced/stale scratch) and snapshot `count`/`bytes`,
never deleting or following links. The packaged client exposes `--runtime-maintenance check|gc|rollback` (rollback requires `--confirm-maintenance`) so users without a source checkout can reach them; UI click-through test still pending |
| Tray, zoom, single instance, diagnostics | Working |
| DSH updates | Real isolated `0.2.0-rc.2` → `0.2.1-rc.2` activation and live runtime/unit rollback verified locally; mock/namespace regressions run in CI. External prefixes are check-only; signed feeds and UI acceptance remain pending |
| Packages | AppImage, deb, tar.gz, PKGBUILD built |
| QR pairing | Loopback warning and non-loopback QR; no one-click exposure |
| System WebKit dependency | Unresolved |
| Native multi-engine | Unresolved |

## Milestones

**Owner acceptance (2026-10-21): M0 and M1 are accepted/closed.** The owner
reports having checked them. Repeat acceptance on a second laptop is deferred
until the milestones are complete, before final release readiness. Historical
validation notes below remain provenance, not a reason to restart M0/M1.
Current implementation priority is M2; signed production delivery is not implied
by acceptance of bootstrap/first-run behavior.

Estimates assume one experienced engineer full-time. Part-time work is roughly
1.5–2× longer.

### M0 — Testable foundation (2–3 days) — owner accepted

- Add an integration harness that runs a fake DSH server on `127.0.0.1:39080`
  and a disposable `DSH_HOME`.
- Test: attach, stale token, missing token, server disappears, child mode,
  window open, diagnostics redaction.
- Add CI: `cargo fmt --check`, `cargo clippy`, `cargo test`, build, package
  smoke tests on a clean runner.
- Add release/versioning conventions and a changelog.
- Never touch the developer's real `~/.dsh` in tests.

**Exit:** `scripts/test-integration.sh` passes without a running real DSH.

**Implemented:** library target, fake-HTTP token-exchange tests, bootstrap
`--check` test, and GitHub Actions CI (fmt, clippy, tests, release build).
Offline bootstrap fixtures now cover PATH without Node/npm, repeat install,
checksum rejection, concurrent installers, and preservation of the previous
harness on npm/install-script/native-load/version failures.

**Additional real-runtime acceptance:** [real_managed_child.rs](../tests/real_managed_child.rs) re-executes
an environment-cleared worker under a private HOME/XDG/cwd and exercises the
production start helper twice using an explicitly selected installed Node/DSH.
Local acceptance passed on 2026-10-20: bare 401/current token 303, old token rejected
after relaunch, marker persistence, private effective child environment and no
orphan cleanup needed on the successful path. A default fixture-only regression
checks the controller's subreaper watchdog against a separate process group.
See [runtime lifecycle testing](UPDATE_TESTING.md#real-managed-child-lifecycle-no-user-service-or-downloads).

**Past internal controlled-child GUI acceptance:** validation passed during
private development on 2026-10-20. This is not public run evidence; verify fresh
public CI against the exact published head. Under Ubuntu 24.04
x86_64/Xvfb, real Ocean/WebKit exercised unavailable-systemd fallback, authenticated
visible-frame beacons, leader-loss recovery, same-window reload and close cleanup,
including same-group tool sentinels. MSRV, native GTK/WebKit and package smoke
steps also passed. This is not a claim of local Xvfb execution or a signed release.
See [CI GUI acceptance](UPDATE_TESTING.md#ci-gui-fallback-and-automatic-recovery-controlled-child-fixture).

**Final cross-laptop recheck:** real DSH HTML/window acceptance and the clean desktop/distro matrix. Neither the real-runtime helper test nor the fake-child
GUI scenario is a model/provider or bootstrap-UI acceptance test. Package/private
install smoke and 0.1.1 preparation have local evidence; hosted publication and
signed release delivery remain separate gates.

### M1 — Bootstrap 1.0 (5–7 days) — owner accepted

- Signed/checksummed runtime manifest: Node.js, DSH, pnpm, architecture,
  channel and minimum versions.
- Atomic runtime layout: `runtime/<version>` + `current` symlink; keep the
  previous runtime for rollback.
- Setup UI: progress, cancel, network error, disk-space check, retry and
  rollback. One click installs a usable DSH.
- Handle npm lifecycle scripts deterministically (`allowScripts` + `rebuild`).
- Optional offline bundle for air-gapped installs.
- Clean-room test: no Node/npm/DSH on the machine, empty `~/.dsh`.

**Exit:** a clean VM installs and opens DSH from the setup window without a
terminal.

**Implemented:** exact Node/DSH/pnpm manifest with Node archive checksums,
private Node PATH, npm lifecycle approval/rebuild, disk check, serialized
installers and repeat-install verification. A complete candidate is installed
under `generations/gen.*`; only the `current` symlink is atomically replaced.
Authenticated health checks (401 then 303) run in a disposable profile on an
OS-assigned port before and after activation. Failure after selection restores
the old selector. `--rollback` validates/selects a predecessor; legacy layout
migration leaves old directories untouched. Discovery and generated units bind
to immutable generation paths, including the matching pnpm.

Offline fixtures cover activation failure, migration, rollback refusal,
selector escape, and preservation of Node/DSH/pnpm together. Node tests cover
health-helper auth, child cleanup, early exits, unsafe URLs, and token redaction.
The opt-in clean-bootstrap workflow exercises the real runtime without system
Node. Neither test path uses the live profile or :3080.

**Acceptance tracking:** first-run/bootstrap is accepted by the owner; repeat
UI/clean-room acceptance on the second laptop is deferred to the final check.
Production signed runtime delivery and running-server restart/rollback belong to
M2; selecting a generation does not change an already running server. The optional
offline bundle is deferred. Existing generation retention/explicit GC need not be
reimplemented.

### M2 — Signed updates and releases (7–10 days) — in progress

- Shell updater for AppImage/tarball with a signed manifest
  (ed25519/minisign), signature checked before download/apply.
- Update channels: `stable` and `next`; explicit current version and channel in
  settings.
- DSH core updates: health-check on an isolated test port first, then atomic
  switch and restart; rollback on failed health check.
- CI publishes signed artifacts and a release manifest to GitHub Releases.
- Package-manager users get a clear “update via AUR/deb” state instead of a
  broken self-update.
- Test tampered manifest, wrong key, interrupted download and bad DSH version.

**Implemented:** a detached generation-update worker for the local generated
user unit, with a shared installer lock, unit snapshot/atomic replacement,
restart and invocation-scoped authenticated HTTP health. Failure restores the
selector and unit and verifies the old service; incomplete recovery retains its
snapshot. UI checks run off GTK, validate channel/version strictly, and no
longer overwrite external prefixes. Completion refreshes auth without a second
restart. Launch tokens are redacted before entering client log/failure state.

**Evidence:** mock service tests cover install/reload/restart/health/cancellation
failures, custom units, stale selection, rollback failure, and private log tails.
An opt-in real user-manager test kills the `systemd-run` waiter and proves the
separate worker still commits (all service commands in that test are mocks).
A separate opt-in real-service test installs `0.2.0-rc.2`, activates
`0.2.1-rc.2`, then rejects a healthy fresh candidate and verifies restoration of
the exact prior unit/selector and authenticated live service. The service is
runtime-linked, uses `env -i` and private HOME/XDG directories, and never uses
port 3080. Production PID is unchanged and its unit is never stopped. This test
passed locally; standard CI compiles it and runs mock/namespace/unit-syntax
regressions. Node tests reject stale tokens, wrong generation, invocation
changes and unsupported unit namespaces.

**Offline shell-release verification foundation:** strict Ed25519 over a
versioned domain prefix plus exact raw JSON bytes, checked before parsing.
A read-only CLI requires an independently provisioned operator public key and
explicit channel/target/kind/sequence policy. It rejects weak keys, malformed or
tampered signatures, unknown/duplicate fields, expired/future/long-lived feeds,
downgrades, low sequences and noncanonical artifact locations. Optional local
artifact checking enforces single-linked regular files, bounded exact size,
SHA-256 and before/after metadata/path checks without changing bytes/modes.
Deterministic test-only keys, Node crypto interoperability and a headless CLI
fixture prove the boundary. No production trust root, download, persistent
rollback state or application of updates exists; existing core/bootstrap paths
are still unsigned. A separate explicit `--stage-release` can stream a local
source into private staging, authenticate size/hash before no-overwrite
publication, and return a read-only descriptor. Source changes and later path
replacement do not change held copied bytes; it does not grant execute permission
or install anything. Fault-injection tests cover interrupted/wrong streams and
late target creation. The read-only verifier rejects staging flags.
An optional offline publisher script now signs exact bytes from an explicitly
supplied owner-only Ed25519 PEM and emits a signature/public-key bundle to stdout.
It creates no keys/files/trust roots/releases and does not validate manifest
policy; Rust re-verification and independent key authentication remain required.
Tests cover unsafe key files/private-output handling and real tool-to-Rust
interoperability using test-only keys. No production signing key was provisioned.
A read-only `--draft-release` derives unsigned metadata for up to four unique
platform/kind pairs, with independent streaming size/SHA-256 and canonical URLs,
sharing verifier policy without creating an authenticated proof. Repeated
`--asset TARGET KIND FILE` groups reject duplicates, mixed input forms and late
file failures without partial JSON; legacy single-input CLI/API remain supported.
Tests exercise all four pairs and the real multi-artifact headless draft → signer
→ strict verifier chain against an independently known fixture key.
It creates no keys/state/files or releases; production publishing is still absent.
See [verification, staging and publisher scope](RELEASE_VERIFICATION.md).

**Signed runtime metadata foundation:** complete per-architecture bundle metadata
has a separate Ed25519 domain/product, exact Node/DSH/pnpm versions, archive/source
checksums and explicit writable-state scope. Runtime trust/channel and private
sequence floor are independent of shell; configured checking and explicit metadata
acceptance never install/execute or access a profile. Cross-product signatures,
replay, malformed/unsafe policy and storage tests pass. Complete signed generation
import now captures/hash-checks/extracts all payload bytes, protects a retained
inventory and holds the shared installer lock without executing/selecting runtime
or advancing the floor. Separate explicit isolated candidate checking validates
native Node, exact signed versions/native bindings and authenticated server health,
with clean disposable state and bounded cancellable process-group ownership.
A separate stopped-writer transaction snapshot primitive now captures/restores
supported full managed-home bytes/modes/mtime/internal links, plugin dependencies
and original absence; sensitive snapshots/candidate retention stay private. It is
not a manual export or a wired automatic rollback. Actual owned systemd/child
stop/switch/restart, restore-before-old-start ordering and pending recovery are
still required; the old bootstrap/core installer is not made signed by these
independent primitives alone. See
[runtime boundary](RUNTIME_RELEASES.md).

**Native shell updater:** distinct tray entries separate core from shell updates.
The GTK dialog provides current/running vs selected-on-disk version, signed
`stable`/`next` channel settings, operator public-key fingerprints, explicit
check/download and confirmed managed apply/rollback/recovery. Separate managed
destination enrollment—not APPIMAGE/path hints—grants local apply authority;
original package executables/desktop entries are never overwritten. Cancellation
and worker gating cover publication/cleanup, and quit defers critical work.
Restart is explicit: quit and reopen the separate managed launcher. A full
isolated GTK fixture exercises both successful releases and retained rollback.
The headless `--shell-status` classifier remains read-only/non-authoritative;
production roots and actual package ownership detection are not invented.
See [scope, guidance and acceptance](SHELL_UPDATES.md).

**Persisted anti-rollback foundation:** `--accept-release` verifies then remembers
`(channel, sequence, release, manifest digest)` in a private 0600 state file under
an exclusive lock; older sequences, conflicting manifests for the same sequence and
semver downgrades are refused, identical acceptance is idempotent, corrupted state
blocks acceptance, and `--release-state`/`--reset-release-state --confirm-reset`
expose and clear it. `--verify-release` stays stateless. This is local
replay/downgrade protection, not a signature, trusted clock or freshness proof.

**Signed shell download:** `--download-release` authenticates the local manifest
and independently provisioned key, enforces explicit policy and the remembered
floor before networking, then streams the signed GitHub artifact URL through
trusted system curl into existing exact-size/SHA-256/no-overwrite private staging.
Curl config is disabled, HTTPS-only redirects/time/size bounds apply, curl errors
cannot publish even a matching body, and early failures kill/reap curl. No key is
fetched, no sequence accepted and no bytes executed. Synthetic-key/fake-curl tests
cover authenticated download → explicit two-version apply → rollback, tampering,
truncation/excess bytes, HTTP/timeout exit, overwrite and pre-network replay refusal.
Real production feed/keys and GUI delivery remain pending.

**Configured feed/trust foundation:** updater settings are stored separately from
Harness/npm configuration, with independent shell/runtime trust and stable/next
channels. Public-key fingerprint confirmation is required for import and explicit
extra confirmation for replacement. Bounded metadata fetches authenticate exact
bytes before policy, never fetch a key or advance a floor. Existing invalid or
unsafe settings fail closed; channel/key changes invalidate held candidates and
expiry/floor are rechecked at use. The native GUI and runtime verifier remain the
next integration steps; no production source is silently configured.

**Local activation foundation:** `--apply-release` activates an already verified
local AppImage or complete tar.gz package into a private owned prefix (no download),
re-verifying bytes after they land, retaining the previous version and recording the remembered sequence
only on success; `--rollback-shell` switches the selector back. No service restart
or process replacement occurs, and this is not a filesystem transaction or
system-wide install. A global floor guard and per-prefix operation lock now span
activation/state commit. Synced selector journals restore originals on normal
errors and require explicit `--recover-shell` after interruption; rollback verifies
retained receipts/bytes and never lowers the accepted floor. This is recoverable
coordination, not an executable health check or a kernel transaction. Tar extraction
is bounded/path/type checked before no-overwrite version publication; the entire
package (including notices) and installed-file inventory are retained. Rollback
refuses damaged payload files or an unexpected file/directory set.

**Remaining:** consistent profile/session backup and data-migration rollback
(runtime/unit rollback alone is not data rollback), production signed feeds/keys,
release CI, signed managed-child updates and real installed-client acceptance.
Native shell channel/settings/update click-through is implemented with synthetic
trust in a private GTK fixture; this is not production acceptance. No live user server has been
updated during these tests. See [real-service validation](UPDATE_TESTING.md).

**Core transaction exit met locally:** update from `0.2.0-rc.2` to `0.2.1-rc.2`
in the isolated test profile, with live runtime/unit rollback proven. M2 is
still incomplete: production signed feed/release delivery and the remaining
safety/UI work above are not implied by this result or the offline verifier.

### M3 — Desktop layer (10–15 days) — in progress

Scope is deliberately not “clone every dsh-tauri plugin”. Deliver the native
features that materially improve the DSH web UI:

- Native task notifications (success/error/needs input) through the session bus.
- Native context menu: copy/paste, open link, save image/file.
- Download handling with a save dialog and a downloads list.
- Profile backup/restore/migration: manifest, checksums, stop server, atomic
  swap, restore on failure. Credentials stay outside the default backup unless
  explicitly selected.
- Plugin overview: installed bundles and versions from the `web` profile;
  deep-link to the official DSH plugin manager for install/remove.
- Session archive/export using the DSH session store; no second session store.
- Worktree, scheduler and archive entries only if backed by existing DSH
  plugins; do not reimplement them natively in this milestone.

**Implemented slice:** profile-configuration snapshots and SHA-256 verification,
CLI creation/verification and explicit offline restore, plus tray backup/open
folder actions. Restore validates untrusted paths/types/limits, stages privately,
checks the target for changes, atomically exchanges directories and retains the
old profile. CLI guards check the user unit, active Ocean socket, configured
HTTP origin, and the runtime operation lock. No packages or scripts are run.

**Evidence:** fixture tests cover tampering, symlink/hardlink/FIFO refusal,
path traversal, limits, concurrent edits/replacement, lock contention, active
controllers and a stale socket file. CLI tests run without Node or a display.
No real user profile or credentials were copied/restored. Native menu
click-through is still pending.

**Downloads slice:** the main webview now uses a native Save chooser, same-origin
HTTP/blob policy, private same-filesystem staging and atomic no-overwrite
publication. A private bounded tray history records metadata, cancellation,
failures, interruption and recovery paths without raw URLs/cookies. WebKit
signals track failures per request; a real ephemeral-context test proves
session-cookie downloads, cancellation, an overwrite refusal, subsequent
success and document-generated blob export. CI runs that test under Xvfb.
A separate actual GTK-dialog test now drives Save/Cancel buttons and the nested
loop, verifies origin-only title/local-only selection/modal parenting, round-trips
a Unicode target and retries an existing name in the same dialog without
changing its bytes or refetching the response. Dialog assertions pass locally;
a separate GTK Broadway disconnect assertion makes Xvfb CI the authoritative
display gate. This is not manual visual acceptance. The history window now
refreshes status changes in place and clears finished metadata without closing
or dropping active rows. Persistence failures are explicit, and saved/recovery
files are not removed. GTK fixture tests cover refresh/clear/close and failure
warnings; rendering is plain text with escaped controls and creation times.
The main navigation fence now follows the exact attached origin, not arbitrary
loopback ports. See [download safety and limits](DOWNLOADS.md).

**Desktop notification foundation:** enabled-by-default generic download notices,
with a persistent tray switch, bounded rates, quiet/transient hints and actions
that only focus Ocean. No filenames, URLs, task content or cookie data enter
D-Bus payloads. A private fake service test covers delivery/actions/disable and
daemon replacement without displaying real desktop banners. A bounded observer
now reads the existing UI event mux, preserving original frames and creating
no new sessions/RPCs. Background running → stopped transitions send only generic
activity, not a claimed successful result. A top-frame isolated WebKit world
forwards a constant signal to an exact-origin-checked Rust bridge. Real
loopback WebSocket → WebKit → private D-Bus tests prove delivery, duplicate and
initial suppression, wrong-origin/type rejection, disabled output and no extra
fixture connections. Contracts were inspected for 0.2.0-rc.2 and 0.2.1-rc.2;
this is not a stable public event API or exactly-once notification mechanism.
See [notification behavior and limits](NOTIFICATIONS.md).

**Native cancellation foundation:** Downloads now has Cancel active transfers,
requesting only a snapshot of current Receiving IDs through a bounded/coalesced
native main-context channel. Exact per-context active-map lookup excludes stale,
future and already-publishing/saved operations. Cancelled status wins over WebKit
failed, cleans owned partial staging and does not post a failure notice. Tests
cover queue bounds, GTK callback filtering and real slow-response cancellation
followed by successful authenticated downloads; no refetch or remote IPC added.
A separate Cancel selected transfer now uses an explicit stable-ID Active
transfer selector, with no default/automatic replacement selection. Current
status is rechecked before sending one ID. Escaped bounded labels and live
refresh preserve identity across metadata changes; terminal selection is cleared
rather than redirected. Pure/GTK tests cover same-name IDs, refresh preservation,
stale status rejection, control enablement and nonselected row retention.
A gated concurrent WebKit fixture additionally holds three authenticated
responses open across two ephemeral contexts, ignores foreign IDs, drives the
real selected GTK action, then lets both remaining streams and a later stale-ID
challenge publish exact bytes. It checks terminal uniqueness, mode 0600, staging
cleanup, no fallback selection and preservation of unrelated files. This is
controlled Xvfb acceptance, not an installed-desktop/engine-matrix guarantee.
The same fixture now enters the actual Save chooser's nested GTK loop, processes
cancellation before a late real Save-button Accept, and verifies that the selected
path is ignored with no destination/recovery/publication. A following transfer
must save normally. Priority fencing permits WebKit to defer terminal emission
until destination handling returns; the test-only watchdog/driver is not a
production auto-accept or cancellation hook. This regression exposed a real
post-chooser second native-cancel SIGSEGV. Isolated CI gdb localized the call;
a one-shot latch set before entering C and terminal/failed guards removed that
repeat. The same fixture then passed under gdb; ordinary CI repeats it without
a debugger. No exact internal WebKit allocation mechanism or full engine-matrix
safety is claimed, and no real user core/data was accessed.
See [download limits](DOWNLOADS.md).

**Volatile byte-progress foundation:** receiving/history rows now show bounded
received bytes and optional server-reported totals, never percentages or a save
claim based on network bytes. Engine updates are throttled to 250 ms without
per-chunk persistence/notifications; terminal callbacks carry final counters.
`serde(skip)` preserves the v1 disk format and resets counters after restart.
Unknown/terminal rows cannot be revived by progress, and finished/cancelled
engine states reject late data. Pure, live GTK and real WebKit slow/cancel/save
checks cover the lifecycle. The window now shows one aggregate, display-only
progress bar (determinate only when every active row reported a plausible total,
otherwise indeterminate); durable byte accounting, resume and throttling remain
absent.

**Native snapshot inspection foundation:** Verify configuration backup now
selects a local snapshot folder (no mkdir), runs existing read-only integrity
checks off GTK under a one-worker RAII gate, and shows only validated profile/
normalized timestamp/counts/bytes in a plain Close-only result. No file content,
per-file paths or raw failure details enter that screen; no restore, automatic
profile/global-store access, service control or package operation is added.
Synthetic valid/tampered/time-privacy, read-only mtime/bytes, concurrent gate and
actual isolated GTK picker/summary checks cover this read-only step, not a restore
wizard or signed/data-schema backup. Aggregate native hash progress now refreshes
at 250 ms with cooperative cancellation at stage/64 KiB read boundaries. A Cancel
request does not release ownership or close the running window before work
returns; blocked I/O/scans are not killed. Cancel versus result commitment is
mutex-linearized, terminal delivery is one-shot, and winning late cancellation
never becomes success. Chunk/final-hash/pre-I/O abort, 32 concurrent commitment
races and real GTK pending/finished window behavior are covered; full Tauri
dispatch/blocked-filesystem acceptance remains pending. No restore cancellation
or per-chunk persistence is added. Metadata enumeration/walk and bounded manifest
reads now also have cooperative checkpoints. Static Opening/Inventory/Hashing/
Rechecking labels distinguish full hash counts from a verified result. Final
inventory equality plus held/reopened manifest/data/root stamps reject detected
changes before success without cleanup/repair. Synthetic scan/read/recheck
cancellation and seven mutation/replacement cases cover these checks; the cases
mutate before the final rescan, not after its last file check. This is stricter
read-only verification (also restore preflight), not an atomic freeze, signing
or protection from malicious same-UID/filesystem/post-check changes.
See [scope](PROFILE_BACKUPS.md).

**Backup inventory and retention:** `--list-profile-backups` reports a
read-only, content-free inventory (name/profile/normalized time/counts) by reading
validated manifests only; `--prune-profile-backups --keep N [--profile NAME]
--confirm-prune` deletes the oldest snapshots per profile through held no-follow
descriptors, fails closed on symlinks/special or multiply-linked entries, keeps
unreadable entries, and leaves other profiles, unrelated files and the active
profile untouched. Interrupted pruning can leave a partially deleted snapshot
(fail-closed and kept). Retention is explicit only — no schedule, archive,
export, signature or recovery of deleted snapshots. A read-only **Backup
history…** tray window shows the same inventory plus whole-data snapshot metadata
(counts/times only, no names/paths/contents) with Close-only buttons; restoring and pruning remain explicit CLI
operations, so the GUI cannot delete or apply anything.
See [scope](PROFILE_BACKUPS.md).

**Session/global storage inventory (also in the tray):** a read-only **Storage
usage…** window shows the same session/shared-store totals with Close only, so the
numbers are reachable without a terminal; it exposes no paths and cannot delete
anything.

**Session/global storage inventory:** `--doctor` reports count-only `sessions` and
`storages` totals (files/bytes, skipped entries, partial-walk marker) from a
no-follow, bounded, content-free walk. No session contents, filenames or keys are
read, and no cleanup/deletion exists. Session/global **snapshots and restore**
remain unimplemented.

**Data-downgrade gate:** snapshots record `clientVersion`/`runtimeVersion`;
restoring older-runtime data into a newer runtime refuses unless
`--allow-data-downgrade` is passed, and the receipt reports the accepted downgrade
plus a warning. Unknown versions never invent a refusal. This is a version
comparison gate, **not** schema rollback/migration.

**Whole-data snapshot foundation:** `--backup-data` writes a `dsh-data` snapshot of
`sessions/` + `storages/` with larger limits, root-prefixed manifest paths,
checksummed copies and counted symlink skipping; `--verify-data-backup` re-reads
structure/checksums/identity, and `--prune-data-backups --keep N --confirm-prune`
re-verifies before deleting. A tray flow asks first (counts/bytes only), shows count-only progress with
cooperative cancel, refuses concurrent runs and copies on a blocking worker (the
worker updates shared counters only; GTK stays on its own thread). Credentials at the home root are excluded by
construction. `--restore-data SNAPSHOT --confirm-offline` restores the roots the
snapshot contains: offline guard before staging and again before each atomic
exchange, per-file re-verification, `RENAME_EXCHANGE` swap and the previous root
retained as `<root>.ocean-retained-<stamp>` (never deleted). It does not migrate
schemas and an interruption can restore one root but not the other.

**Export coverage for configuration snapshots:** a round-trip test exports a
profile snapshot, verifies the archive, imports it and checks that the engine
accepts the copy and that a re-export is byte-identical. The systemd renderer also
has unit tests for its loopback and exposed variants.

**Backup export:** `--export-backup` writes a deterministic ustar archive
(manifest + files, fixed metadata, no overwrite) and `--verify-export` re-checks
every hash against the embedded manifest. Exports are unsigned: they prove
integrity, not authorship, because the client holds no key or trust root.

**Backup import:** `--import-export` extracts an archive into a staged sibling
directory, re-checks every hash against the embedded manifest, runs the owning
engine's verification, and only then renames it into place; a refused archive
leaves nothing behind. Authorship is still not proven.

**Backup export UI:** the tray offers **Export a backup…** with snapshot and
destination choosers, a manifest-only confirmation, a blocking worker with cancel
and a one-run gate; refused/failed exports remove the partial archive.

**Pairing page contract:** the same stubbed-IPC approach drives `ui/remote.html`:
state rendering, the hidden URL box before exposure, the rendered URL/QR after the
backend reports one, and `close_pairing` on **Close**.

**Page/command drift:** every `invoke("...")` in the pages is checked against the
`tauri::generate_handler!` list, so an unregistered command is a CI failure.

**Page/payload drift:** a pure test extracts every `state.<field>` read by
`ui/index.html` and `ui/remote.html` and checks it against the serialized
`SetupState`/`RemoteState`, including the nested server/runtime fields.

**Setup IPC contract:** a native WebKit test stubs `__TAURI_INTERNALS__.invoke`
and drives the real page script, asserting the command names, the absence of any
action before the user asks, and that cancellation stays reachable while
`start_bootstrap` is in flight. A renamed backend command breaks CI.

**UI asset rendering:** a native WebKit test loads `ui/index.html` and
`ui/remote.html` and asserts the expected controls plus the pairing access-control
note, catching a broken asset without a desktop session. These page tests run in their own CI step, each in a private Xvfb/D-Bus
session, and load the pages from a throwaway loopback HTTP server (a `file://` load
was intermittently slow to finish on CI runners). Each binary runs in its own private
native session and gets one identical retry after a first failure, because WebKit's
first load on a fresh runner is occasionally slow; a genuine failure fails both
attempts. They do not drive the real Tauri window or
the Harness page behind it.

**Launch smoke:** CI starts the real binary under a private Xvfb display for a
bounded time, fails on a panic or an early exit, and requires `xwininfo` to see a
mapped `DSH Ocean` window, and requires a second launch to hand over to the running
instance and exit, leaving exactly one window — covering tray/window/WebKit startup
and single-instance handover without a service. It does not click through the UI and does not prove
rendering of the real Harness in a desktop session.

**Independent review pass:** a fresh-context review of the backup/export/staging/
pairing modules found 18 issues; the high-severity ones are fixed with regression
tests (per-engine file/entry caps, creation-time retention order, export privacy,
import of empty directories and exec bits, proxy descriptor release). Remaining
gaps are listed in `docs/PROFILE_BACKUPS.md` under *Engine limits and known gaps*.

**Plugins overview:** a read-only tray window lists the profile's bundle names and
dependency versions (no paths, contents or actions). Deep links and other desktop
integrations remain unimplemented.

**Backup verification sweep:** `--verify-backups` verifies every profile and
whole-data snapshot in the backup root and lists `verified`/`failed` without
repairing or deleting anything.

**Backup staging collection:** an explicit, age-gated
`--collect-backup-staging` removes only the engines' own
`.ocean-stage-*`/`.ocean-data-stage-*`/`.ocean-data-restore-*` directories after a
read-only `--list-backup-staging` preview; symlinks are never followed and kept
entries are reported. Published snapshots and live data are untouched.

**Remaining:** session/global-data snapshots, profile migration/restore wizard,
backup retention/export/signing, real-task/desktop-daemon notification acceptance
and wider transport compatibility, context-menu acceptance,
manual Save-chooser interaction, download resume/throttle controls and installed-client/transport multi-stream acceptance,
plugin overview/deep links and the other desktop integrations. Configuration
backup is best-effort when writers are active and is not data-schema rollback.
See [profile snapshot scope](PROFILE_BACKUPS.md).

**Exit:** notifications, context menu, downloads, profile backup/restore and
plugin deep links work end-to-end.

**Read-only pairing plan:** `--pairing-plan` reports the configured origin, whether
it is loopback-only, the discovered LAN address, the exact service override and
client `serverUrl` a person would have to apply, plus exposure/revocation warnings.
It **changes nothing**: no bind address, firewall rule, config value or service is
touched, and a missing LAN address never produces a half-written command. This is
guidance, not the one-click exposure wizard or a revocation workflow.

**Exposure preparation:** `--prepare-exposure [IP]` goes one step further without
crossing the safety line: it stages a candidate unit behind
`--host 0.0.0.0 --trusted-host <lan:port>` (0600, private state directory),
refuses public/loopback/link-local/unspecified addresses and a unit whose port
does not match, and prints the apply, verify and revoke commands. It never runs
`systemctl`, edits the installed unit or opens a port; applying stays a human
action. QR pairing, one-click trust, TTL/revocation UI and the loopback-to-LAN
proxy below remain unimplemented.

### M4 — Pairing and mobile web (7–10 days)

- **Pairing URL through the forwarder:** while it is active the QR points at the LAN
  address and proxy port with the TTL note; expired/revoked/loopback-bound or
  LAN-less states advertise nothing.
- **Core implemented (CLI only):** `--pairing-proxy` forwards a separate LAN port to
  the loopback server with an explicit TTL, revoke, a connection cap and no logging;
  `--check` proves binding without exposing anything. The pairing window now starts and stops it (5/15/60 min) with a countdown and a
  live connection count. Still missing: a TTL-bound QR,
  QR with expiry, per-client identification, TLS and revoke from the pairing window.

- One-click “Pair a phone” from the tray.
- Preferred implementation: a short-lived loopback-to-LAN proxy bound to a
  separate port, with the DSH server still on `127.0.0.1` and the proxy
  authority registered as a trusted host. Closing the pairing window kills the
  proxy.
- QR contains a one-time URL with a TTL; UI shows expiry and a revoke button.
- Auto-revoke on timeout, app quit or explicit stop.
- PWA path: DSH web already ships a manifest; verify installability and add
  “Add to Home Screen” guidance. No native Android/iOS app in 1.0.
- Optional SSH-tunnel wizard for remote users who do not want LAN exposure.
- Phone test matrix: Chrome Android, Safari iOS, LAN only; no Cloudflare by
  default.

**Exit:** phone scans QR, opens a working session, and access is dead after
revoke/expiry.

### M5 — Portable runtime and WebKit fragility (10–15 days)

- Choose Flatpak as the primary portable distribution: GNOME runtime provides
  WebKitGTK and GTK, so a system WebKit upgrade cannot break the app.
- Resolve lifecycle inside the sandbox: either systemd user service via the
  session bus or an in-app child runtime; document the trade-off.
- Keep AppImage/deb/AUR as system-WebKit builds with an explicit dependency
  check and a clear error if WebKitGTK 4.1 is missing.
- CI matrix: clean Ubuntu, Fedora and Arch containers/VMs; install, bootstrap,
  start, update.
- Diagnostics report WebKitGTK/GTK versions and ABI compatibility.
  **Implemented (read-only):** `--doctor` prints `gtk=… webkit2gtk-4.1=… (ok|too
  old|missing|unknown)` probed via `dlopen`, without starting GTK/WebKit. It does
  not bundle WebKit, choose an engine or repair a missing library.

**Portable tar build foundation:** the binary tar builder now always invokes a
locked Cargo build and selects the root client's JSON-reported executable,
including custom target directories; it does not reuse a guessed stale binary.
Fixed repository inputs/documentation, no-follow parent/file FDs, native ELF
sanity checks and manifest-change checks precede normalized tar/gzip creation.
Owned scratch/output FDs, atomic no-overwrite link publication and fsync avoid
old-output deletion or silent replacement. Synthetic stale/cross-target/failure/
symlink/FIFO/size/version-change/collision/reproducibility tests plus an actual
tar/headless-version smoke cover the build, not installation or portable ABI.
All CI Cargo build/test/clippy stages are locked. This is not a signature,
reproducible Rust build, source attestation or production trust root.

**Debian archive foundation:** Debian now shares the same locked Cargo/descriptor/
identity/no-overwrite publisher, with no reused staging directory. A deterministic
Debian 2.0 ar contains control/data and all-file md5sums; native architecture,
SemVer `~` prerelease + `-1` revision, absolute desktop Exec, license/notices and
normalized modes/times are explicit. No maintainer scripts, portable installer
or service actions are included. Synthetic metadata/checksum/reproducibility/
collision/failure tests plus real ar/dpkg-deb info/contents and private-home
headless-version smoke cover archives only, not installation. Dependencies are
fixed host-native metadata, not ELF-derived ABI minima.

**AppImage tool-gate foundation:** implicit mutable-cache/continuous execution is
removed. Fetch requires an explicit HTTPS URL + independent operator SHA-256,
checks each redirect before following, and publishes a bounded non-executable
private digest-named cache without replacement. Build requires explicit tool/path
and digest, verifies an owned regular single-link source and executes only its
private checked snapshot. The shared locked Cargo/FD/no-overwrite backend creates
fresh private AppDir staging with normalized modes/mtime and bounded native ELF/
type-2 header checks. Synthetic tool/response tests cover missing/bad trust,
extraction/build/header/symlink errors, cache/output collisions and downgrade
rejection. Real AppDir/AppRun private-home headless smoke covers staging without
third-party execution. This is **not genuine squashfs/AppImage acceptance**: no
real independent tool digest or production publisher root is selected. The
explicitly trusted tool is not sandboxed and may acquire its own runtime/network
inputs. Genuine-tool build/inspection, manual launch and the distro/Flatpak/install
matrix remain pending. See [packaging scope](PACKAGING.md).

**Portable new-install foundation:** the tar now bundles a Python 3.11+ install-
time helper (not a runtime dependency). It refuses root, relative/control paths,
source/target symlink ancestors and any existing target. Source root/file FDs
are pinned and checked; all inputs/conflicts are preflighted before staging.
Seven new user files use exclusive staged/pinned inodes, fsync and per-file
no-clobber links. Caught failure removes only matching created inodes, preserving
replacements and reporting incomplete cleanup. No service/cache helper or GUI
launch occurs. Desktop Exec uses double-layer quoting/literal percent encoding,
checked by GLib. Synthetic races/defaults/alias tests and a real trusted-tar
install only under disposable private HOME/XDG cover this fresh-install step.
A successful install now atomically publishes a private 0600 ledger (seven
absolute paths with mode/size/SHA-256, no credentials/timestamps). `--uninstall`
requires it, refuses tampered/outside-prefix paths, verifies every recorded file
(component no-follow, regular single-link, size+hash) **before** deleting anything,
and keeps everything when the verification pass rejects anything; empty-directory
cleanup is limited to directories this install created strictly inside the owned
prefixes (adopted shared XDG trees survive). No upgrade/downgrade, retained old
version, signature/provenance authentication or atomic multi-file transaction is
implied; the ledger is same-user state. Existing-target migration, persistent
rollback, real-user desktop/launcher/cache and clean-distro acceptance remain
pending; source-build legacy installers are separate. See [installation scope](INSTALLATION.md).

**Exit:** a clean system without Node/DSH runs DSH Ocean from Flatpak, and the
same machine can update WebKit independently without breaking the client.

### M6 — Engine adapters (research 3–5 days + 10–20 days)

“Native multi-engine” is not a small feature: DSH already owns providers and
plugins. Decide between:

1. **Adapter route (recommended):** manage installed Codex/Claude CLIs and
   expose them through existing DSH provider plugins. Add engine detection,
   auth status, version compatibility and per-session selection. No engine
   binaries bundled.
2. **Native route:** implement providers inside the client. This duplicates DSH
   and is a 4–8 week project with ongoing upstream compatibility work.

Start with a spike: inspect `dsh-codex-subscription`, the DSH provider API and
the CLI auth model. Gate the implementation on a written design and a working
one-engine prototype.

**Exit:** Codex or Claude is selectable and usable end-to-end, with explicit
auth/version errors; the design documents why the other route was rejected.

### M7 — 1.0 hardening (5 days)

- Security review: token handling, LAN pairing, navigation fence, update
  signatures, diagnostics redaction.
- Recovery paths: corrupt profile, failed update, missing WebKit, full disk.
- Documentation: install, Flatpak, pairing, updates, troubleshooting.
- Tag `v1.0.0`, publish signed release assets.

## Sequencing

```text
M0
├── M1 ── M2
│    └── M5
├── M3
├── M4
└── M6 (spike can start after M0)
M7 after M1–M6
```

M0/M1 are owner-accepted; M2 is the current implementation priority. M2 and M5
both depend on the bootstrap foundation. M3 and M4 can run in parallel with M2.
M6 is gated by its spike. Final cross-laptop acceptance stays deferred, not waived.

## Testing strategy

- Unit tests for token parsing, version comparison, QR generation, navigation
  fence.
- Integration harness with a fake server and disposable `DSH_HOME`.
- Clean-room bootstrap and update tests in containers/VMs.
- Manual test matrix for tray, close-to-tray, Wayland/X11, phone pairing.
- Release verification: signature, checksum, install, launch, update,
  uninstall.
- No test may touch the developer's live `~/.dsh` or port 3080.

## Risks

| Risk | Mitigation |
|---|---|
| WebKitGTK ABI breaks on distro update | Flatpak primary; version diagnostics; CI matrix |
| LAN pairing exposes a bearer token | TTL, explicit consent, proxy only, revoke, no public tunnel by default |
| systemd unavailable in Flatpak | Child-runtime mode inside the sandbox, or session-bus integration |
| DSH upstream changes profile/plugin API | Pin supported DSH range; smoke tests against `next`; adapter boundaries |
| Update bricks the only runtime | Atomic layout, previous runtime kept, health check, rollback |
| Codex/Claude auth and licensing | Adapter route; user-owned CLI/auth; no bundled credentials |
| Profile lock during backup/restore | Stop server, lock file, atomic swap, restore backup on failure |

## Definition of done for 1.0

On a clean machine with no Node, npm or DSH:

1. Install DSH Ocean from a signed Flatpak or package.
2. First launch bootstraps a private, checksummed runtime.
3. The client starts/attaches to DSH and opens the official UI.
4. It updates the shell and the DSH core through a signed channel, with
   rollback.
5. A phone pairs through a QR and loses access on expiry/revoke.
6. Profile backup/restore and diagnostics work without exposing tokens.
7. At least one external engine is selectable through a supported adapter.
8. `cargo test`, integration tests, package smoke tests and the clean-room test
   pass in CI.

## Immediate next sprint

1. M2: complete signed shell delivery on top of the existing verifier, download,
   local AppImage activation and explicit rollback. Add authenticated feed/key
   provisioning and user-facing orchestration without treating test keys as trust.
2. M2: connect signed runtime delivery to existing isolated health checks and
   generation/unit rollback; preserve profile/data and external-install boundaries.
3. Keep each change in a focused PR with isolated tests. Never update the live
   client/server/profile or publish a release as part of implementation tests.
4. Continue M3–M6 in the sequence above. Recheck owner-accepted M0/M1 on the second
   laptop after the milestones are complete, before declaring final readiness.
5. Release only after explicit authorization and authenticated inputs: production
   trust, independent AppImage digests, required license/source payloads, package
   checksums and signed artifact verification. No invented keys, tags or releases.
