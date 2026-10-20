# Changelog

All notable user-visible changes to DSH Ocean. The client version lives in
`Cargo.toml`; `dsh-ocean --version` prints it.

Format: one section per released version, newest first. Unreleased work is listed
under **Unreleased** until a version bump and tag. Entries describe behaviour, not
internal refactors, and never include profile contents, tokens or credentials.

## Unreleased

- Source builds select a checksum-verified local backport of the upstream
  RUSTSEC-2024-0429 / GHSA-wrw7-89jp-8q8g iterator fix for GTK3-compatible
  `glib 0.18.5`. The version is not disguised as 0.20; version-based alerts may
  remain. CI verifies the exact source delta and actual optimized iterator values.
  Crates.io publication is disabled because root Cargo patches do not propagate
  to registry consumers. This is not a complete dependency/license audit.

- Service preparation, manual restart, health recovery and runtime update now share
  one in-process admission gate. Overlapping requests are refused as busy rather
  than queued; stale health observations and obsolete window reloads are discarded.
  Update ownership includes reauthentication after success or rollback. Window-only
  retries reuse an already owned healthy child instead of spawning a second one.
  This is not cross-process coordination or rollback of cancelled external work.

- Managed-child restart stops the owned process group before reaping its leader,
  including when the server leader has already died. This prevents same-group
  tools from surviving automatic recovery. If stopping cannot be confirmed, the
  old child handle is retained and a replacement is not started in that attempt.
  Processes that deliberately escape the owned group are outside this guarantee.

## 0.1.1

Stabilization release prepared on 2026-10-20. Distribution signing and publication
remain separate; this section is not a claim of a signed production release.

- Token-cache, remembered-release-state and backup-export manifest reads reject
  FIFO inputs without waiting for a writer; bounded regular-file checks remain.
- Source installs no longer reintroduce unsupported desktop URL arguments and
  quote executable paths correctly. Source builds and Make targets use the locked
  dependency graph. The AppImage Make target requires an explicitly trusted local
  tool and digest instead of attempting an unconfigured download.

- Inactive unit setup preserves/refuses differing, linked or special installed
  units instead of overwriting them; missing units publish complete bytes without
  clobbering a racing name. Runtime rollback reports that differing pinned units
  require explicit operator review/replacement. Automatic updates normalize only
  one generated PATH directive; other existing custom directives refuse updates.
- Diagnostics collection and disk writes run on one background worker. Hidden
  setup windows stop polling; visible state requests no longer overlap, including
  during installer cancellation.

- Idle single-instance socket clients can no longer block all subsequent window
  activations indefinitely. Requests have a one-second total read deadline and
  only a complete `show` command is honored; malformed/partial requests do not
  raise the window. Native smoke also tests reactivation with an idle peer.

- Runtime-maintenance execution failures now exit 1 without a misleading usage
  hint; malformed arguments remain usage errors with exit 2. Installer exit
  statuses continue to pass through unchanged.

- Attachment to an already running, authenticated DSH server now happens before
  local runtime detection. It works without a local Node/Harness installation
  and leaves the installed service unit unchanged; runtime installation and
  service setup are reserved for actually starting a local server.

- GUI startup and runtime rollback now enforce checked server configuration
  before service/helper side effects. Existing malformed/rejected configuration
  no longer becomes a localhost default; a valid explicit environment URL still
  takes precedence. An unavailable remote origin cannot trigger local service
  startup, and doctor never sends cached credentials to a diagnostic fallback.
- Backup directory opens strip trailing separators and `/.` before applying
  `O_NOFOLLOW`, closing Linux's terminal-symlink bypass without canonicalizing
  or following the link.

- `closeToTray: false` now quits Ocean through normal shutdown when the main or
  setup window closes; default/true still hide, and auxiliary windows never quit
  the app. The legacy `dshVersion` setting is explicitly reported by doctor as
  unsupported: bootstrap remains pinned by the embedded runtime manifest.

- Opening links and folders now starts and waits for `xdg-open` on a worker,
  preventing unreaped direct children without blocking the GTK thread. Opener
  output is not copied into client logs, where authenticated URLs could leak.

- `--verify-backups` no longer treats unreadable/missing roots as successful empty
  sweeps or skips snapshots with broken manifests. Recognizable damaged or
  symlinked candidates appear in `failed`; per-snapshot failures still print JSON
  but now exit 1. Existing empty directories continue to pass. Nothing is mutated.

- Non-UTF-8 command-line arguments now return a clear usage error instead of
  panicking; unknown arguments are not echoed into the terminal. Desktop
  launchers no longer advertise unsupported file/URL arguments (`%U`), including
  portable installs, Debian, Arch and AppImage builds.

- Empty/relative HOME, XDG and DSH directory overrides no longer place user
  configuration/state in the current working directory. Invalid HOME falls back
  to the account database; other invalid directory overrides use absolute defaults.
  Session locks/sockets and restore guards share the same runtime-directory rule.

- Configuration (64 KiB) and runtime rollback-marker (1 KiB) reads now reject
  symlinks, special files, invalid UTF-8 and oversized contents, preventing FIFO
  hangs during startup/doctor. Unreadable configuration is distinguished from
  missing configuration without discarding a valid environment URL override.

- Exposure plans now derive the candidate and exact revocation copy from the
  actual installed unit, not a newly rendered approximation. Missing, symlinked,
  oversized or mismatched units fail closed. Each preparation uses a fresh
  private plan directory and exclusive files, preserving earlier revoke copies.
  IPv6 plans use bracketed authorities and an IPv6 bind address.

- Empty/whitespace-only `DSH_OCEAN_URL` values no longer suppress `serverUrl`
  from the configuration file; runtime selection and doctor use the same rule.
  HTTPS is explicitly rejected because the native probe/attach transport has no TLS.
  Pairing-proxy, exposure preparation and restore guards refuse invalid explicit
  origins instead of acting on the display/doctor loopback fallback.
- Server URL overrides now accept HTTP origins only, with no credentials,
  non-root path, query, fragment or zero port. Invalid overrides fall back to
  loopback and are flagged by `--doctor`. Diagnostic/doctor URL rendering also
  strips credentials and URL suffixes defensively.

- Diagnostic plugin metadata now uses the same filtered names/version list as
  the plugin window, excluding private dependency URLs and arbitrary objects.
  Profile metadata reads are capped at 1 MiB and reject symlinks/non-regular
  files without blocking on FIFOs.
- Diagnostic reports no longer collect unrelated applications’ user journals;
  new reports use private directories/files (0700/0600), unique names and
  exclusive creation. The save dialog now warns about private service-log and
  plugin metadata instead of claiming all report contents are anonymous.

### Added

- Native, read-only verification of configuration snapshots with aggregate byte
  progress and cooperative cancellation; the progress window never hides while
  work is still running.
- Read-only backup history window and `--list-profile-backups` inventory
  (counts/times only).
- `--prune-profile-backups --keep N [--profile NAME] --confirm-prune` snapshot
  retention with fail-closed traversal.
- Portable tar installation writes a private ledger; `--uninstall` removes only
  size/mode/hash-matching files and keeps everything on a mismatch.
- Cooperative cancellation of the runtime installer from the setup window, with
  an honest `Committed`/`Cancelled`/`Failed` outcome and process-group cleanup.
- `--runtime-maintenance check|gc|rollback` for installed clients, plus stale
  scratch sweeping and `--gc` runtime generation collection.
- `--doctor` reports read-only runtime generation/scratch and backup counters.
- `--accept-release`, `--release-state`, `--reset-release-state`,
  `--apply-release` and `--rollback-shell`: remembered release sequences, local
  activation into a private prefix, and rollback to the retained previous version.
- Deterministic, no-overwrite portable tar and Debian archives built from the
  locked Cargo artifact; AppImage requires an explicit SHA-256-pinned tool.

- Tray **Storage usage…** shows session/shared-store totals read-only (Close only,
  no cleanup).
- CI lints every shell script with `shellcheck -S warning`.
- `--doctor` accepts `--json` for machine-readable output and `--strict` to exit 1
  on a blocking problem (unusable WebKitGTK ABI or an unreachable server).
- `--doctor` reports the loaded GTK/WebKitGTK versions and whether they match the
  built `v2_40` feature level.
- `--doctor` also reports count-only session/shared-storage totals (no names or
  contents, no cleanup).
- `--pairing-plan`: read-only phone pairing/exposure plan with the exact override a
  person would apply; nothing is changed.

- Downloads window shows an aggregate progress bar for active transfers, determinate
  only when every active row reported a plausible total.

### Changed

- CI now verifies the declared Rust 1.90 MSRV by building every target on that
  exact toolchain.

- Debian version mapping uses `~` for SemVer prereleases and an explicit `-1`
  revision.
- Every non-`--check` runtime install sweeps leftover scratch before starting.

- Profile snapshots record the client/runtime versions, and restoring older-runtime
  data into a newer runtime requires an explicit `--allow-data-downgrade`.

- `--backup-data`, `--verify-data-backup`, `--prune-data-backups`: snapshot and
  retention for `sessions/` and `storages/` (scope `dsh-data`, skipping symlinks,
  counting them).
- Tray **Back up sessions and stores…** with a count-only confirmation, a
  count-only progress dialog with cooperative cancel, a blocking worker and a
  one-run gate.
- `--list-data-backups` and a whole-data section in the read-only backup history
  window (metadata only).
- `--restore-data --confirm-offline`: restore those roots with an offline guard,
  atomic exchange and the previous root retained.

- The diagnostics bundle now includes the same content-free runtime/storage/ABI
  counters as `--doctor`.

- `--doctor --json` lists runtime generation sizes and whether each is referenced.

- `--list-backup-staging` / `--collect-backup-staging --older-than-minutes N
  --confirm-collect`: find and remove abandoned snapshot staging directories only.

- `--prepare-exposure [IP]` stages a private candidate unit for LAN exposure and
  prints apply/verify/revoke commands; it never restarts the service.

- `--doctor` reports the effective configuration source and flags a missing,
  unreadable or unparseable `config.json`, URL overrides and unknown channels.

- Tray menu ids, labels and dispatch come from one exhaustive action table, so a
  dead menu entry is a compile error instead of a silent no-op.

- `--export-backup SNAPSHOT DEST.tar` / `--verify-export ARCHIVE.tar`: deterministic,
  unsigned single-file exports re-checked against the snapshot manifest.

- `--import-export ARCHIVE.tar DEST_DIR` extracts a verified export into a new
  snapshot directory (staged, never partial), after which engines verify it.

- Tray **Export a backup…**: snapshot picker, destination picker, count-only
  confirmation, blocking export with cancel, no overwrite.

- `--verify-backups [DIR]` verifies every snapshot in the backup root read-only and
  reports verified/failed lists without repairing or deleting anything.

- CI launches the real binary under Xvfb and requires it to stay alive without a
  panic **and** to map a `DSH Ocean` window (`scripts/smoke-app-launch.sh`, private
  XDG dirs, unroutable origin), and that a second launch hands over, exits by itself
  and leaves exactly one window.

- CI renders the shipped setup and pairing pages in a real WebKit view and asserts
  their controls and access-control note.

- CI asserts the setup page's IPC contract with a stubbed
  `__TAURI_INTERNALS__.invoke` (state, prepare, install, cancel, diagnostics, quit).

- CI also asserts the pairing page's contract (state rendering, hidden URL box, refresh,
  close) against a stubbed IPC surface.

- A drift test compares the `state.<field>` reads in both pages with the serialized
  setup/remote payloads, so a renamed field fails immediately.

- A second drift test checks that every command the pages invoke is registered in
  the Tauri handler list.

- `--pairing-proxy --ttl-seconds N [--port P] [--check]`: short-lived transparent
  LAN forwarder to a loopback server with TTL, revoke, connection cap and no logging.

- The pairing window can start and stop the short-lived LAN forwarder with a
  countdown and a live connection count (`start_pairing_proxy`,
  `stop_pairing_proxy`).

- While the forwarder runs, the pairing QR/URL use the discovered LAN address and
  proxy port with a remaining-time note; nothing is advertised when it is inactive.

- `--doctor` reports whether the cached launch token is still accepted (read-only
  exchange; the token is never printed).

- Profile snapshots now have export/import round-trip coverage, and the unit
  renderer has tests for both the loopback and exposed variants.

- Export reading is size-bounded per member (1 GiB) and per archive (8 GiB) before
  allocating, so a crafted archive cannot exhaust memory.

- Reviews of the backup/pairing modules fixed: whole-data files are no longer held
  to the 16 MiB configuration limit, retention orders by creation time, exports are
  created 0600 in a 0700 directory, imports recreate empty manifest directories and
  keep the executable bit, proxies release finished connections, and manifest
  directories are validated like file paths.

- Restore now reports partially replaced roots, uses a fallback retain name, and
  restore leftovers are never collected automatically; export has a real cancel
  point; `--prepare-exposure` prints runnable commands with absolute paths.

- Closed the remaining review gaps: expired forwarders are replaced on start,
  manifest matching is indexed, manifests are read O_NOFOLLOW and bounded, and the
  staging collector removes through held descriptors.

- Data snapshots fsync every copied file and directory, and record whether the
  source changed during the copy (`consistent`), which the tray result surfaces.

- Exports and imports fsync the destination directory, so a success is durable.

- Tray **Plugins…** window (read-only bundle/dependency list), and three recovery
  fixes: rollback feeds its health helper, rollback refreshes the stopped unit, and
  `--gc` keeps the generation the installed unit pins.

- Runtime/service hardening: the status poll runs off the GTK thread with one
  cached, time-bounded detection; an existing server is only adopted when it
  answers 401; the readiness line must be a loopback http origin; the managed
  child owns its process group and its descendants are killed on stop/quit; and
  `$` is escaped in ExecStart so a path containing one still starts.

- Recovery now restarts a wedged-but-active `dsh-web.service`, and the installer's
  process-group id and `installing` flag are cleared on every exit path; shutdown
  probes the group before signalling it.

- CI page fixtures now realize the WebKit view in a mapped window with compositing
  and software GL pinned, addressing the intermittent page-test timeouts that the
  single retry used to absorb.

- Security: every app command is now bound to the window that owns it, so script
  running in the harness page can no longer open a LAN forwarder, quit, restart or
  start a bootstrap; the launch-token cache is written atomically 0600 in a 0700
  directory with O_NOFOLLOW, and is read with a link and size check.

- UI and session robustness: download history is persisted outside the shared
  mutex (no fsync on the GTK main thread), the setup log re-renders when its text
  changes instead of freezing at the ring-buffer cap, cancelling a snapshot or
  export keeps the window that reports the outcome, the pairing page stops polling
  while hidden, and a single `flock` now guards the desktop session.

- Added round-trip, trimming, corruption and abandoned-temp tests for the download
  history file, covering the gap the UI review noted.

### Security

- Installer and archive builders refuse existing/racing outputs, symlinked
  inputs and detected source changes instead of overwriting them.
- AppImage tool acquisition rejects insecure redirects before following them and
  keeps cached tools non-executable.

## 0.1.0

Initial native client: attach to or supervise a local DSH server, systemd and
managed-child modes, tray, diagnostics, native downloads, first-run runtime
bootstrap with generation rollback, and AppImage/deb/tar packaging.
