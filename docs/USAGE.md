# Using DSH Ocean

For installation and project status, see the [README](../README.md).

## Quit the application

Choose **Ocean → Quit DSH Ocean** in the window menu, or press **Ctrl+Q**
(**Cmd+Q** on macOS). The command is translated into Ocean's selected language
and remains available without a system tray. Closing the window still follows
the close-to-tray preference. Quitting waits for active installer cleanup; it
does not interrupt a transaction while it is committing.

## Runtime maintenance

The installed client can report, collect and roll back its private runtime
without a source checkout:

```bash
dsh-ocean --runtime-maintenance check     # read-only prerequisites/selection
dsh-ocean --runtime-maintenance gc        # stale scratch + unreferenced generations
dsh-ocean --runtime-maintenance rollback --confirm-maintenance
```

`gc` keeps the selected generation plus one rollback hop; see
[runtime maintenance limits](ARCHITECTURE.md#stale-work-collection).
It is not automatic retention or a backup.

### Portable tarball

Use only a trusted local build or independently authenticated release. Extract it
into a new private directory with a trusted tool, inspect/navigate into its exact
versioned root—never select an installer with a shared `/tmp` wildcard.
The [portable installer](INSTALLATION.md) requires Python 3.11+ at install time
(not to run the client), refuses root/existing targets and does not launch helpers,
services or the application. A successful install writes a private 0600 ledger;
`--uninstall` removes only ledger-recorded files whose size, mode and hash still
match, verifies them all before deleting anything, and prunes only directories the
install created. Upgrades/overwrite remain separate work.

```bash
# From the exact trusted extracted package root:
./install.sh --check      # read-only preflight
./install.sh              # new files + private ledger
./install.sh --uninstall  # remove only hash-matching recorded files
```

## First run

1. The client prefers the selected private runtime under
   `~/.local/share/dsh-ocean/current`, with legacy `harness` layout support,
   then checks `~/.local/opt/dsh` or `dsh` on `PATH`.
2. If no server is listening on `127.0.0.1:3080`, it writes and enables
   `~/.config/systemd/user/dsh-web.service` and starts it. If systemd is not
   usable, it starts a managed child process on an OS-assigned loopback port.
3. It reads the authenticated URL that `dsh web` prints to journald, verifies
   the token exchange, and opens the DSH UI with a signed HttpOnly session
   cookie.
4. If DSH is missing, the setup window offers a one-click private runtime
   install (Node.js, DSH, pnpm) without touching the system Node.js.

The enabled unit starts DSH at the next login even if the client is closed. If
you prefer the old behavior, disable it:

```bash
systemctl --user disable dsh-web.service
```

### Runtime generations and rollback

New private installs use `generations/gen.*/{runtime/node,harness,tools}`.
All three components are verified before an atomic `current` symlink switch.
An isolated DSH starts on an OS-assigned loopback port before and after the
switch; failed post-activation checks restore the previous selector. The
health helper uses a disposable HOME/profile, never the live profile or :3080.

Legacy directories are not moved or overwritten. A successful migration records
`legacy` as its predecessor; rollback can return to it. To select the previous
runtime, run from the source checkout:

```bash
bash scripts/bootstrap-runtime.sh --rollback
```

Use the same `DSH_OCEAN_DATA_DIR` if you installed under a custom prefix.
Rollback checks the previous runtime before switching. It does **not** restart
running servers or rewrite active systemd units. The client resolves immutable
generation paths, so existing processes keep their own files and pnpm. Old
generations are retained; automatic garbage collection is not implemented.
This provides atomic selection and process-failure rollback, not a guarantee
against filesystem corruption or power loss.

### Generation-aware in-app updates

For a private generation served by local `dsh-web.service`, the tray update
confirmation starts a separate `systemd-run --user` worker. It holds the runtime
lock across installation, unit replacement, restart and authenticated health.
The helper verifies the running generation, PID and current invocation's launch
token. Failed activation restores both the old selector and the old unit, then
restarts/checks the previous service. If recovery fails, a snapshot is retained
and the client reports incomplete rollback rather than success. Concurrent unit
edits are preserved rather than overwritten during activation or recovery.

The worker survives closing the GUI; logs are private files (mode `0600`) under
`~/.local/state/dsh-ocean/updates`. The GUI reauthenticates after completion and
does not restart the service twice. Health recovery/manual tray restarts are
paused in that client while its update is running. Custom unit changes, stale
selection, remote origins and managed-child updates are refused before mutation.
The detached worker requires a systemd version supporting
`systemd-run --expand-environment=no`.

External npm prefixes are now check-only: use their original package manager.
Ocean does not modify them in place; safe adoption into a private generation
remains to be implemented. No signed Ocean update feed or shell self-updater is
claimed here. **Rollback currently restores runtime and unit only, not profile
or session data migrations. Configuration snapshots do not cover shared session
or storage data; plan separate backups before updating.**

Past internal validation passed real isolated user-service `0.2.0-rc.2` →
`0.2.1-rc.2` activation and live fault-injected rollback, including
current-invocation 401/303 auth. This is not a public CI result; verify the
current public head and its checks separately.
Its HOME/XDG paths, runtime tools, service name and port are separate from the
user's service. Ordinary CI keeps this networked test opt-in and exercises fast
mock regressions instead. Native confirmation/reload click-through is still
pending. See [real-service test instructions and isolation](UPDATE_TESTING.md).

## Profile configuration backup and restore

Use **Back up web configuration…** in the tray, or:

```bash
dsh-ocean --backup-profile web
dsh-ocean --verify-profile-backup /absolute/path/to/snapshot
# Only in an offline maintenance window, after quitting Ocean and all writers:
dsh-ocean --restore-profile web /absolute/path/to/snapshot --confirm-offline
```

Snapshots live under `~/.local/state/dsh-ocean/profile-backups`, with private
permissions and per-file SHA-256. Global credentials, sessions, storage data and
dependency bytes are outside the scope. Custom profile files may themselves
contain secrets. Live snapshots are best-effort, not a global data snapshot.
Restore never stops a service for you or installs/runs dependency scripts; it
refuses active local controllers and retains the replaced profile for recovery.
See [scope, safety limits and maintenance instructions](PROFILE_BACKUPS.md).

## LAN pairing (opt-in)

The client never exposes DSH by itself; loopback is the default. If you need a
phone or another machine on a trusted LAN, expose the server deliberately:

1. Point DSH at the LAN interface in
   `~/.config/systemd/user/dsh-web.service`:

   ```ini
   ExecStart="…/node" "…/dsh" --profile web --host 0.0.0.0 --port 3080 \
     --trusted-host 192.168.1.10:3080 --no-open
   ```

2. Reload and restart the unit, then point the client at that authority in
   `~/.config/dsh-ocean/config.json`:

   ```json
   { "serverUrl": "http://192.168.1.10:3080/" }
   ```

   `serverUrl` (and the `DSH_OCEAN_URL` override) must be a plain HTTP origin:
   no username/password, non-root path, query or fragment; the port must be
   nonzero. HTTPS is not supported by the native connection probe. Blank
   `DSH_OCEAN_URL` values are ignored, preserving the configured URL.
   Do not paste an authenticated launch URL here. Invalid values use
   the loopback default for display/doctor; `--doctor` reports the fallback
   without echoing secrets or sending cached credentials to that fallback.
   GUI service startup, rollback, pairing-proxy, exposure preparation and restore
   commands refuse invalid explicit origins or invalid existing configuration
   before acting. A valid nonblank environment URL takes precedence over the file.
   An unavailable remote server never triggers local service startup or restart.
   The configuration must be a regular, non-symlink UTF-8 JSON file, at most
   64 KiB; unreadable or rejected files use display defaults, are reported by
   `--doctor`, and block service-changing actions unless a valid environment URL
   explicitly selects the origin.

3. Restart the client and choose **Pair a device…** from the tray. The QR code
   contains a bearer URL that grants full harness access. Use only a trusted
   network and remove the `--host 0.0.0.0`/`serverUrl` changes when finished.

The server has no TLS; do not do this on an untrusted network.

## Switching a legacy unit

If an older DSH unit is still loaded and serving port 3080, switch it from a
normal terminal (not from inside the DSH chat):

```bash
# Replace the example with the exact unit you intend to stop.
DSH_LEGACY_UNIT='legacy-dsh.service' scripts/switch-to-new-service.sh
```

The script stops the selected old unit, starts `dsh-web.service`, checks that
systemd reports it active, and prints instructions for inspecting its journal.
It does not verify authenticated HTTP readiness or print a launch URL itself.
Keep any token-bearing journal output private. It does not hardcode the legacy name.
Stopping the old unit disconnects the current web session; sessions remain on
disk.

## CLI

```text
dsh-ocean --version       client version
dsh-ocean --runtime       detected DSH/Node runtime as JSON
dsh-ocean --doctor        runtime, server and unit status, plus read-only
                          runtime generation/scratch and backup counters
dsh-ocean --print-unit    print the generated dsh-web.service
dsh-ocean --help
```
