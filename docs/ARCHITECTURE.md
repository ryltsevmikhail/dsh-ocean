# DSH Ocean architecture

The client has one job: make the official DSH web UI feel like a native
application without duplicating the harness or weakening its security posture.

## Process model

```text
dsh-ocean (Tauri/GTK)
├── setup window          local tauri:// assets
├── main window           http://127.0.0.1:3080 (official DSH UI)
├── tray                  StatusNotifierItem / libayatana-appindicator
└── background tasks
    ├── prepare service
    └── health monitor

dsh web (node)            127.0.0.1:3080
└── DSH profile web       ~/.dsh/profiles/web
    └── sessions, plugins, providers
```

The web server is deliberately not a child of the window by default. The
default path is a systemd user unit (`dsh-web.service`) so closing the client
does not kill live sessions. If systemd is unavailable, a managed child is
created with `kill_on_drop(true)`.

## Startup state machine

1. **Single instance.** A kernel-held `flock` owns the desktop session; the socket
   `$XDG_RUNTIME_DIR/dsh-ocean.sock` is only an activation channel. A second
   process sends `show\n`, focuses the existing window and exits. Only the complete
   five-byte command is accepted, with a one-second total read deadline. An idle
   or malformed peer is ignored instead of blocking later activations indefinitely.
2. **Setup window.** A local Tauri page renders state from
   `commands::get_setup_state`, scheduling the next poll 700 ms after completion.
   State requests never overlap; hidden setup windows stop polling and refresh
   when shown again. Installer cancellation remains independent of state polling.
3. **Prepare.**
   - Validate the configured origin before side effects, then probe it with a
     minimal HTTP/1.1 request, before requiring any local runtime.
   - **Attached:** an existing server answered `401`. Resolve a launch token
     from journald/cache, validate it with a non-followed `GET /?token=…`
     expecting `303`, and reuse the server. No local Node/DSH installation is
     required; attachment neither writes nor enables a systemd unit.
   - **Systemd:** no server was found at a loopback origin (an unavailable remote
     origin fails without local fallback). `bootstrap::detect()` now finds Node
     and DSH and checks their version. `systemd::ensure_unit` publishes a missing
     `~/.config/systemd/user/dsh-web.service` from complete private staging without
     replacing an occupied name, then reloads, enables and starts it. An inactive
     existing unit must be a bounded, single-linked regular file with identical
     bytes; different content, symlinks and special files are preserved/refused.
     Active units remain untouched. Review and explicitly replace differing units
     outside Ocean; a runtime rollback does not silently rewrite their pins.
     The token is polled from `journalctl -u dsh-web.service`.
   - **Child:** systemd is unavailable or failed. Run
     `node <dsh> --profile web --host 127.0.0.1 --port 0 --no-open`, read the
     printed authenticated URL, and keep the child in `AppState`.
4. **Window.** `windows::open_main` creates the Tauri main window with
   `WebviewUrl::External(token_url)` and an `on_navigation` fence. The DSH
   server exchanges the token for its signed HttpOnly cookie and redirects to
   `./`.
5. **Ready.** The setup window is hidden, the tray remains available, and a
   health monitor probes the server every 10 seconds.

## Private runtime generations

A candidate generation contains Node, the DSH npm prefix and pnpm together.
The installer validates versions/native modules and runs an authenticated HTTP
health check with a disposable HOME and OS-assigned port. `current` is replaced
by a same-filesystem symlink rename; failed post-activation checks restore the
old selector. No live server or profile is used by these checks.

Discovery resolves `current` once and returns the immutable generation paths.
The generated systemd unit and PATH therefore keep one generation's Node,
DSH and tools even if selection changes. Old generations remain on disk for
those processes. Legacy directory layout still works when no selector exists;
malformed or escaping selectors are rejected rather than silently used.

The updater never npm-installs into an immutable generation or external prefix.
For the generated local user unit, it starts a separate user-manager worker with
file-backed output and literal environment expansion disabled. The worker owns
the bootstrap flock through installation and service activation. It snapshots
and compares every line of the old unit, normalizing only one syntactically
valid generated `Environment="PATH=…"` directive (other custom changes are refused),
then renders the new
generation into a pre-escaped unit template, replaces/reloads/restarts the unit,
and verifies its current invocation, PID, ExecStart and authenticated HTTP.
Failure restores the selector and unit before restarting/validating the old
service. A failed recovery retains a private snapshot and is reported as such.

The worker survives GUI exit. An in-process guard prevents duplicate updates
and pauses that client's health recovery/manual restart actions. Completion
only reattaches the window, without another restart. External prefixes,
managed-child and remote updates remain unsupported. Bootstrap selection alone
still does not restart a server or update an active systemd unit.

Real-service validation has a separate, explicit test-unit namespace. It uses
runtime-linked unit files, a private HOME below the test prefix, strict runtime
PATH and `env -i` for service/installer processes. The DSH service gets private
XDG runtime sockets; only its health worker keeps the user-manager bus path.
Default production environment and unit name remain unchanged. Fragment identity compares
canonical paths so a runtime link can refer to the private regular unit file.
The ordinary updater entry point cannot select a test unit, and the isolated
entry point cannot select the production unit. Unit syntax is checked before
starting a real test service. See [validation details](UPDATE_TESTING.md).

## Native downloads and navigation

The main webview is fenced to the currently attached server origin, including
when child-mode restarts change its port. Other loopback apps and same-host
scheme/port changes are not delegated automatically to a different cookie jar.
External ordinary links still open outside the webview; local privileged setup
content is not an allowed main-view navigation target.

A direct WebKit-context signal bridge retains the browser's authenticated
session and gives every download its own failure/cancellation state. A native
chooser supplies a new target; its GTK-only builder/runner is independently
testable without Tauri or user folders. Occupied names remain in the same
chooser with an inline warning, preserving the original response. The backend
still rechecks races. WebKit writes only to private staging beside the target. Completion publishes with `RENAME_NOREPLACE`, never partial/overwriting writes
to the selected destination. Metadata is bounded and private; raw URLs, cookie
headers and WebKit error strings are not persisted. Native history has no
remote IPC/file-execution surface. Its GTK-only live view polls bounded shared
metadata, never user files, with weak widget references that stop after close.
Clear finished keeps active rows/the window open and warns if persistence fails;
no file deletion or durable-clear claim is hidden behind a failed save.
See [download scope](DOWNLOADS.md).

## Desktop notification delivery

The native notifier sends only fixed enum-selected freedesktop payloads, not
web content. It is asynchronous, rate-limited and enabled by default after
loading the app preference; its transport starts inert until configured.
Download terminal events use it; cancellation/blocking do not. The tray setting
persists in a separate private preference file without rewriting main config.

Notification IDs are paired with the daemon's unique D-Bus owner. Replacement
IDs, close calls and action validation cannot accidentally target a restarted
service's unrelated reused ID. Recognized actions only focus Ocean. Disabling
gates new/queued sends and closes owned replies best-effort. Private-bus tests
use no real desktop notification daemon.

Task activity observes only the existing main-frame same-origin event mux.
Original WebSocket sends/constructor behavior remain intact; no new connection
or RPC is created. A bounded page-memory state machine recognizes ready streams
and true → false status transitions, resetting on disconnect/generation change.
Only a constant DOM event enters a top-frame isolated world and a constant
numeric signal reaches Rust. The native handler is absent from the page world,
checks the current exact server origin, and never stringifies arbitrary values.
Preference, foreground suppression and rate gates apply before D-Bus delivery.
This unprivileged notice capability does not widen Tauri remote IPC permissions.
The inspected 0.2.0/0.2.1 mux contract is not a stable public API; missed events
and unsupported transport overrides remain limitations. See [notification scope
and test evidence](NOTIFICATIONS.md).

## Profile configuration snapshots

The snapshot engine uses directory-descriptor-relative, no-follow reads and
bounded regular files; it excludes dependency/Git trees and never reads global
credentials or session stores. SHA-256 and an exact inventory verify imported
snapshots. File and tree metadata are compared across copy; live consistency is
still explicitly best-effort. Snapshots are private and not signed/encrypted.

Manual restore acquires profile and runtime locks, checks offline conditions
before staging and before activation, and refuses detected target changes.
Linux `renameat2(RENAME_EXCHANGE)` replaces the whole directory atomically; the
replaced profile is always retained rather than deleted. There is no unsafe
two-rename fallback. Dependencies are not installed and scripts are not run.
An explicit operator maintenance window is required because other DSH CLI
writers do not honor Ocean's locks. No automatic updater data rollback is
claimed. See [profile backup format and limits](PROFILE_BACKUPS.md).

## Token handling

`dsh web` prints exactly one authenticated URL at startup:

```text
dsh web: http://127.0.0.1:3080/?token=<base64url>
```

The token is a process-scoped bearer credential. `auth`:

1. scans `journalctl --user` (unit first, then a bounded global tail) for the
   newest `dsh web:` line;
2. parses only the `token` query parameter;
3. validates it by requesting `/?token=…` and requiring a `303` without
   following the redirect;
4. caches it in `~/.local/state/dsh-ocean/web-token` with mode `0600`.

Diagnostics call `auth::redact` on all journal/systemd text. The token is never
serialized into `SetupState`, logs or window titles.

## Failure modes

| Failure | Behavior |
|---|---|
| DSH not installed | Setup window shows the runtime installer. `bootstrap-runtime.sh` downloads a checksum-verified Node tarball and `npm install`s DSH + pnpm under `~/.local/share/dsh-ocean`. While it runs the window shows a **Cancel install** control and refuses a second concurrent install. |
| Server running, token missing | Setup shows a retryable error. The client never kills a server it cannot authenticate. |
| systemd start fails | Log the error and fall back to a managed child. |
| Managed child dies | Health monitor calls `restart_and_reload`; a new token is read from the new process. |
| systemd unit dies | Health monitor starts the inactive unit; systemd also has `Restart=on-failure`. |
| Window closed | Hide to tray; server keeps running. |
| Second launch | Unix socket focuses the existing window. |

## Runtime installer cancellation

`start_bootstrap` takes a one-shot slot for the whole command (installer plus
the service preparation that follows), so a second invocation returns the current
snapshot instead of starting a competing download. Only a *started installer
child* can be cancelled: the slot and the cancellation bit live in one mutex, so
a click that arrives before the spawn or after the child was reaped returns
`false` and cannot rewrite an outcome. Starting an installer clears any stale
request in the same critical section, so a late click cannot cancel the next run.
Across processes, the script's own `flock` on `.bootstrap.lock` remains the guard.

The child runs as its own process-group leader, so cancellation reaches the
script and a blocking `curl`/`npm` descendant. `src/bootstrap_supervisor.rs`
sends `SIGTERM` once to that group, lets the script's `TERM`/`EXIT` traps run, and
escalates to `SIGKILL` after a 20 s grace period. The setup window keeps a
usable **Cancel install** control while `start_bootstrap` is still awaiting (it
does not reuse the shared `busy` guard), shows **Cancelling… (may wait for I/O)**
once accepted, and re-renders from `installing`/`cancelRequested` snapshots.

Outcome classification matches what the script can actually guarantee:

* the installer's `INT`/`TERM` traps are commit-aware — once `COMMITTED=1` a
  signal exits **0**, so a late cancel cannot rewrite a committed install into a
  reported cancellation;
* an exit success is therefore always `Committed`;
* a signalled non-zero exit is `Cancelled`;
* anything else is `Failed`.

The app does not claim that cancellation "restored the previous selection": after
a cancellation it re-runs `bootstrap::detect()` and logs either that a runtime was
already selected before the signal (retry the service start) or that nothing was
selected. `reset_idle` keeps the retry reachable; when a runtime is present the
install control stays hidden because there is nothing to install.

After the child is reaped the supervisor **sweeps its group** once more with a
bounded `TERM`/`KILL` escalation. A fast-exiting script can otherwise leave a
descendant holding the output pipes or the inherited runtime lock; the command
never awaits those pipes for more than five seconds.

Limits, stated plainly:

* `SIGKILL` (or app exit/`panic=abort`/`std::process::exit`) bypasses the script's
  traps. A forced stop can leave `generations/gen.*`, `.download.*` or
  `.pointer.*` behind, can leave the selector pointing at a generation selected in
  the seconds-wide window between `select_target` and `COMMITTED=1`, and can leave
  a `curl`/`npm` descendant holding `.bootstrap.lock`; nothing sweeps those
  directories automatically. Reinstall then reports "another bootstrap is already
  running" until the survivor is gone. Nothing outside the private runtime prefix
  is touched.
* Application shutdown only *asks* the installer group to stop (`SIGTERM`) and
  then kills the managed server; it cannot wait for cleanup, and an immediate
  `process::exit` skips even that request.
* Runtime **updates** (`runtime_update.rs`, the systemd worker) are not
  cancellable through this control.
* Cancellation is cooperative: bash defers traps until the current foreground
  command returns, so a stop may wait for one `curl`/`npm` invocation, and the
  bounded grace period is what eventually forces it.

### Stale-work collection

A forced stop cannot run the script's traps, so it can leave the installer's own
transient scratch (`$PREFIX/.download.*`, `$PREFIX/.pointer.*`) or an unselected
`generations/gen.*` directory behind. Two bounded, opt-in-safe mechanisms exist:

* Every non-`--check` run first sweeps `.download.*`/`.pointer.*` **direct
  children** of the prefix. The runtime `flock` is already held, so no other
  bootstrap can own them; any exact generated name is stale. A non-directory,
  symlink or otherwise unexpected entry makes the run fail closed instead of
  being deleted.
* `dsh-ocean --runtime-maintenance gc` (or `bootstrap-runtime.sh --gc` from a
  checkout) removes stale scratch and every `generations/gen.*`
  directory **except** the currently selected generation and the one named in its
  `.previous`, i.e. the runtime keeps exactly one rollback hop. It prints counts,
  refuses a symlinked `generations` directory, never follows or deletes anything
  outside `$PREFIX`, and leaves unrelated prefix content (`harness`, `runtime`,
  `tools`, `current`, notes) untouched. It does not verify that a removed
  generation is unused by a *running* server; run it when a server started from
  that generation is not needed.

The packaged client exposes the same modes without a source checkout:

```bash
dsh-ocean --runtime-maintenance check    # read-only prerequisites/selection
dsh-ocean --runtime-maintenance gc       # stale scratch + unreferenced generations
dsh-ocean --runtime-maintenance rollback --confirm-maintenance
```

`check` and `gc` never restart or reconfigure anything. `rollback` changes the
active runtime, so it refuses to run without `--confirm-maintenance` and then
relies on the script's own health checks and selector rollback. The installer text
is fed to `bash -s` on stdin (never a temporary file or the process list), the
private prefix comes from the same resolution the app uses, and the child inherits
the terminal so its log stays visible. Unknown or malformed modes exit with status
2 before creating anything.

The **Save diagnostics** action writes the same content-free environment facts
(runtime generation counters, backup counts, session/storage totals, WebKitGTK ABI)
alongside versions, service status, plugin metadata and redacted service-unit
journal lines, so a support bundle does not need a terminal. Unrelated user
journals are excluded. Reports are exclusively created as 0600 files with unique
names; new report directories use 0700. Session contents are not collected, but
service logs and plugin metadata may contain private information: review reports
before sharing. Token redaction is not a general-purpose secret scrubber.

`dsh-ocean --doctor [--json] [--strict]` prints one read-only report: runtime,
server URL and HTTP status, service state, unit/config paths, runtime generation
counters, backup counts, storage counts and the loaded WebKitGTK ABI.
`--json` also lists each `generations/gen.*` directory with its file/byte size and
whether `current` or its recorded `.previous` still references it, so a person can
see what `--runtime-maintenance gc` would remove before running it. Sizes come from
a bounded, symlink-free walk and are a lower bound when the walk stops (`truncated`). so
support scripts do not have to scrape text. `--strict` exits **1** when a part the
user cannot work around is broken — the WebKitGTK ABI is `missing`/`too old`, or
the configured server answered nothing at all — and still prints the report; it is
intended for scripts and checkers, not as a startup gate. `launch token` reports whether the cached launch token is still accepted by the
configured origin (`accepted` / `rejected` / `not verified` / `no cached token`).
The check performs the same read-only index exchange the client itself uses and
never prints the token.

It also explains where the server origin came from: a missing, unreadable or
unparseable `config.json` is reported instead of silently falling back, an
nonblank `DSH_OCEAN_URL` override is named as such, and an unsupported or invalid
origin or an unknown `updateChannel` is listed as an issue. Blank environment
values are treated as unset. Only plain HTTP origins are accepted: the native
attach/probe transport does not implement TLS, so HTTPS is explicitly rejected
rather than reported as a valid but mysteriously unreachable server.

`--doctor` itself never
downloads, installs, cleans or restarts anything.

`dsh-ocean --doctor` reports the **runtime ABI** the host actually loads:
`gtk=<version> webkit2gtk-4.1=<version> (ok|too old|missing|unknown)`, resolved
with `dlopen`/`dlsym` and *without* initialising GTK or WebKit. The client is built
against WebKitGTK 4.1 with the `v2_40` feature, so a WebKitGTK older than 2.40 is
reported as `too old` and an absent library as `missing` (the window will not
start); the line is diagnostic only and never installs or upgrades anything.

**Pairing/exposure preparation:** `--prepare-exposure [IP]` (built on the read-only
`--pairing-plan`) stages a **candidate** unit file under the private state
directory (`exposure/<unique-plan>/dsh-web.exposed.service`, mode 0600) that binds
`--host 0.0.0.0` (IPv4) or `--host ::` (IPv6), with an explicit trusted authority,
and prints the exact apply, verify and revoke commands. The installed unit is
read once as a regular, non-symlink file capped at 64 KiB. Its exact bytes are
saved next to the candidate for revocation; no runtime is detected or unit
re-rendered. Only a single, unambiguous loopback ExecStart with the configured
port is accepted. Each plan has a new private directory, so preparing again never
overwrites an earlier revocation copy. It refuses public, loopback, link-local
and unspecified addresses, refuses a unit whose port does not match, and never
calls `systemctl`, writes the installed unit or opens a firewall port. IPv6
URL/trusted authorities are bracketed. Anyone who can reach the port sees the
same UI; the Harness session token is the only access control.

Tray entries are a single `Action` table: menu construction, the event handler
and the id lookup all use it, and the dispatch match is exhaustive, so a menu
item cannot exist without a handler and a typo cannot create a silently dead
entry. Ids and labels are asserted unique and stable by unit tests.

The tray's **Storage usage…** window renders the same counters (sessions, shared
stores, skipped entries, partial-scan note) with **Close only** — no delete,
cleanup or move control. Deleting session data stays an explicit operator action
outside the client; the window says so and never forwards a filesystem path.

`dsh-ocean --doctor` also prints read-only **storage** counters for the DSH home
(`sessions` and shared `storages`: file counts and total bytes, plus skipped
entries and a partial-walk marker). The walk never follows a symlink, never reads
file contents, never reports a file name, and stops at bounded entry/depth limits
instead of silently truncating. There is no delete/cleanup action here.

`dsh-ocean --doctor` also prints read-only counters for this prefix
(`selected`, `previous`, `known`, `unreferenced`, `staleScratch`) and for the
snapshot folder (`count`, `bytes`). Counting never deletes, follows a symlink or
prints a path inside the profile; an unreadable selector or generations directory
is reported as `unavailable (…)` instead of guessing.

Neither mechanism is automatic retention policy or a backup: there is no schedule,
no archive/export, and no record of when or why a generation was created.

Supervisor behaviour is covered with real short local processes: cooperative
`TERM` with a cleanup trap, ignored `TERM` escalating to `KILL`, a surviving
grandchild swept after a fast successful exit, a late cancel not rewriting a
committed exit, and a plain failure. A separate offline test drives the **same
bytes the app embeds** (a guard fails if `scripts/bootstrap-runtime.sh` and
`include_str!` ever differ) with a fake blocking `curl`, cancels it during the
download, and asserts both staging directories were cleaned, no selector was
activated and the private `HOME` stayed empty. Further offline tests cover the
scratch sweep before a download, `--gc` retention (selected + one rollback hop
kept, orphans and scratch removed, unrelated prefix content and the `current`
selector preserved), the no-generations no-op, `--help`, and refusal of a
symlinked `generations` directory.

## Offline release authentication seam

A separate read-only verifier authenticates a domain-separated, exact-byte
Ed25519 shell manifest before parsing, then enforces caller-supplied policy and
optionally hashes a local single-linked regular artifact. Its CLI runs before
GUI/runtime/service initialization and never downloads or applies anything.
The public key/floor are explicit operator inputs, not an embedded trust root
or persistent anti-rollback store. URL metadata is pinned to the repository;
a future downloader/apply transaction still needs staging, redirect, snapshot
and rollback policy. No Tauri capability exposes this file-reading surface.
Existing core/bootstrap update paths remain unchanged and unsigned.
See [format, evidence and trust limits](RELEASE_VERIFICATION.md).

## Tauri surface

- `capabilities/default.json` grants `core:default` to the local setup/main
  windows only.
- No remote capability is exposed, so the DSH page cannot call application
  commands.
- Zoom is handled by a GTK key-press handler on the WebKit view, not by remote
  IPC.
- Navigation is fenced in `windows::navigation_allowed`.
- The tray and native dialogs live entirely in Rust.

## Files

| File | Role |
|---|---|
| `src/main.rs` | Tauri builder, CLI, window-close policy, single instance, health monitor |
| `src/state.rs` | process-wide state, setup snapshot, child handle, quitting flag |
| `src/service.rs` | attach/systemd/child state machine, restart, health recovery |
| `src/auth.rs` | journal token discovery, validation, cache, redaction |
| `src/bootstrap.rs` | Node/DSH detection, runtime paths and the pinned manifest |
| `runtime-manifest.json` | Node archive checksums and DSH/pnpm pins used by the app bootstrap |
| `src/systemd.rs` | neutral unit generation, enable/start/restart, status |
| `src/windows.rs` | setup/main windows, navigation fence, zoom shortcuts |
| `src/tray.rs` | tray menu and actions |
| `src/diagnostics.rs` | redacted JSONL bundle |
| `src/update.rs` | npm dist-tag check and core update |
| `src/single_instance.rs` | Unix socket focus protocol |
| `scripts/bootstrap-runtime.sh` | private Node/DSH/pnpm installer |

## Runtime process invariants

The managed DSH child is spawned in its own process group, so stopping it kills
the shells and tools it started. The setup window polls detection off the GTK
main thread, with one cached probe per three seconds and a two-second bound on
`dsh --version`; a hung shim therefore cannot freeze the UI. The readiness line
printed by the harness is treated as untrusted: only a plain `http` loopback
origin with a real port and the root path is accepted. An existing server on
the configured port is adopted only when it answers `401`, the same contract
`check-runtime.mjs` enforces. Recovery restarts a unit that is active but no longer
answers, and the installer lifecycle (process-group id plus `installing`) is
cleared by a drop guard on every exit path, with the group probed before any
shutdown signal is sent.

## Page fixture notes

The shipped setup, pairing and asset pages are exercised in CI through a real
WebKitGTK view served over loopback HTTP from an ephemeral context. The fixture
realizes the view in a mapped window (`DSH_OCEAN_TEST_MAP_WEBVIEW=1`), because
script execution on an unmapped view is unreliable on a bare Xvfb display; the
job also disables compositing and forces software GL. Each binary still runs in
its own private D-Bus/Xvfb session, because one shared session leaves the second
WebKit web process unable to finish loading. CI keeps a single retry per page
binary as a safety net; that retry is not evidence that a first failure was
innocent.

## IPC trust boundary

The `main` window loads the harness origin, so page scripts running there — a
third-party profile bundle, a session artifact — share the injected Tauri bridge.
Tauri re-checks a plugin's ACL but not a non-plugin command's origin, so every
command that can change state takes `webview: WebviewWindow` and names the window
allowed to call it: `setup` for detection/bootstrap/diagnostics/quit/restart,
`remote` for the pairing forwarder and its state. Tray actions call the internal
functions directly, so nothing is lost. `tests/page_payloads.rs` fails if a
mutating command stops being bound.

## UI threading and session ownership

Saving diagnostics collects subprocess/filesystem facts and writes the bundle on
one blocking worker, never in the GTK callback. A process-local RAII gate coalesces
repeated save requests until the worker returns; only the result dialog is posted
back to the main thread. This prevents UI blocking, not a timeout guarantee for
unresponsive external tools or filesystems. A pure test covers gate ownership;
full installed-client dispatch acceptance remains separate.

Download history is changed under its mutex but written and fsynced outside it,
so a slow state directory cannot block WebKit callbacks on the GTK main thread.
Progress windows keep a single source of truth in shared state: cancelling only
sets a flag, the timer keeps rendering, and the window closes once the terminal
outcome is on screen — the WM close button requests the same cooperative stop
instead of destroying the view of a running copy. The setup log compares its
rendered text, because the log ring buffer is capped. Session ownership is a
`flock` on `dsh-ocean.lock`; the socket is a convenience, so a second launch can
never unlink a live instance's socket.
