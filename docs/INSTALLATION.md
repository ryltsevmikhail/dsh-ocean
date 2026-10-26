# Portable tar: new user installation only

The tar's `install.sh` runs its bundled `install.py`. This helper requires Python
**3.11+ at installation time**; the installed Rust client does not require Python.
It runs as the current non-root Linux user. It does not launch the client, contact
DSH/model services, enable/restart systemd, update desktop/icon caches, or run
external helpers. Debian/AppImage do not include either portable installer file.

## Trust and scope first

This is **not an untrusted-archive extractor, signature verifier or sandbox**.
Running an installer already authorizes that code. Use your trusted local build,
or verify external release authenticity independently before extraction/execution.
ELF/header and filesystem checks do not establish publisher identity. Do not run
as root/sudo, use an old shared extraction directory, or select installers with
wildcards such as `/tmp/dsh-ocean-*`. Extract only a trusted archive into a new
private directory using a trusted archive tool, inspect its one versioned root,
and navigate into that exact root yourself.

```bash
# From that exact trusted package root, not a wildcard/shared directory:
./install.sh --check  # Read-only preflight; creates no directories/files.
./install.sh          # New files only. Existing targets always cause refusal.
```

Defaults are `$HOME/.local/bin/dsh-ocean`, five icon files below
`$XDG_DATA_HOME/icons/hicolor`, and the desktop file below
`$XDG_DATA_HOME/applications`. Unset/empty XDG_DATA_HOME falls back to
`$HOME/.local/share`; unset/empty DSH_OCEAN_BIN_DIR to `$HOME/.local/bin`.
Overrides must be absolute, contain no control characters/parent traversal and
use no symlink ancestors. Source invocation with parent traversal is also refused. Existing final parent directories must be owned and not group/world-
writable. Missing parent directories are created private; no existing directory
permissions are changed. System prefixes and existing files/symlinks/directories
are not replaced. An existing ledger also blocks installation, so a second
install is refused. **There is no force/overwrite/upgrade option.** Use a separately
chosen fresh private prefix for acceptance testing; do not delete an old install
blindly to get past a refusal.

## Filesystem contract

The source root is acquired component-by-component from `/` with no-follow
directory opens and held once across source-file acquisition. Invocation preserves
source spelling rather than resolving away aliases. Root identity is rechecked;
source symlink ancestors, symlinks/special/multiply-linked files, detected changes,
oversized input and mismatched native ELF fail. File limits are 512 MiB for the
client, 16 MiB for each other input. This is host/header sanity, not ABI/distro
compatibility or a frozen/authenticated filesystem snapshot.

All seven sources and every target conflict are preflighted before staging.
Each new file is copied through held source FDs into an exclusive random scratch
file in its held, owned target parent. Mode and contents are staged/fsynced before
publication; source metadata/path identities are compared again. Hardlink
publication is **per-file atomic no-clobber**, including concurrent/dangling
symlink/directory collisions. No existing target is opened for writing. Client
mode is 0755; icons/desktop 0644. Scratch inode FDs remain pinned through cleanup.

On an ordinary caught failure, published files are removed **only if their
inodes are still the ones this invocation created**; replacements are preserved
and incomplete cleanup is reported. Owned scratch cleanup is attempted for all
records even after another cleanup refusal. Empty directories may remain.
This is **not an atomic seven-file transaction, persistent rollback journal or
power-loss-safe upgrade**. A crash can leave a partial new installation; concurrent
same-UID edits to created inodes, malicious filesystem behavior and failures
during rollback/fsync are not prevented. Inspect the prefix after failure; never
assume complete rollback or apply an indiscriminate cleanup script.

## Install ledger and `--uninstall`

A successful installation atomically publishes one private (0600) ledger at
`$XDG_STATE_HOME/dsh-ocean/installed-portable.json` (unset/empty falls back to
`$HOME/.local/state`). It records exactly the seven absolute paths this invocation
published, with the mode actually applied to each final file (from `fstat`, not
the requested mode), its size and SHA-256 of the final bytes. It also lists only
the directories this install created strictly inside the owned prefixes, so
uninstall can prune exactly those. It contains no credentials, profile data,
hostnames or timestamps, but the recorded absolute paths do reflect the
HOME/`DSH_OCEAN_BIN_DIR`/XDG_* values used at install time. If the
ledger cannot be published, the whole installation is rolled back as above.

`--uninstall` requires that ledger, and must be run with the **same**
`XDG_STATE_HOME`/`XDG_DATA_HOME`/`DSH_OCEAN_BIN_DIR` values as the install (or
with both unset, matching the defaults), otherwise the ledger is not found or its
paths fall outside the current prefixes. It does **not** delete arbitrary files. It
re-reads each recorded path component-by-component without following symlinks,
requires a regular file of the recorded size, mode and hash, and verifies
**everything before removing anything**. If any recorded file was modified,
replaced, chmodded, turned into a symlink or has an unexpected type,
the verification pass fails without deleting anything: every file and the ledger
are kept. A tampered ledger
that names a path outside the current binary/icon/data prefixes is refused before
any check. Only after full verification are the recorded files unlinked (each
re-checked against its verified metadata immediately before `fsync`), then only
directories this install created and recorded, once they are empty. Directories
that already existed — including a shared `~/.local/share/icons/hicolor` or
`applications` tree, even if uninstall leaves them empty — are never removed, and
neither is an unrelated file placed next to the installer's files.

Removal is per-file; an interruption between unlinks, or a mismatch/IO error
detected during the deletion pass, can leave a partial installation with the
ledger still present, which the next `--uninstall` can continue (missing files are
skipped, unowned files refused). The "keeps everything" guarantee applies to the
verification pass, not to a failure that happens after deletion has begun. A
same-user swap between the final check and `unlink` is inherent to path-based
removal and is outside the non-hostile model.

This ledger makes removal *size/mode/hash-verified*, not authenticated: it is same-user
private state, so a same-UID actor could rewrite it. It is not a provenance
signature, package-manager database or proof about how the payload got there. It
does **not** implement upgrade, downgrade, migration between prefixes, or
restoration of a previous version; those require a separately designed retained-
backup/rollback flow. Deleting the ledger alone makes `--uninstall` refuse; remove
files manually only after inspecting them yourself.

## Desktop entry and existing installation boundaries

The absolute executable path is a quoted Exec argument with both desktop string-
value and argument escaping; percent characters become literal `%%`. There is no
`%U` field: the client does not accept desktop file/URL arguments. No `sed` or
shell evaluation is applied to a path. Existing desktop attributes are otherwise retained. Tests
use an independent GLib key-file/argv reader for spaces, quotes, ampersands,
dollars, backticks, percent, apostrophe and backslash. The reader does not launch
the desktop entry.

No upgrade/apply flow, retained old version, provenance/signature ledger,
backup/rollback migration or running-process maintenance window is implemented
here. Source-build `scripts/install.sh`/`scripts/uninstall.sh` are separate legacy
operators, not made transactional by this change. No original DSH profile,
credentials, sessions or user service is part of this payload. The newly installed
client's later first-run/lifecycle behavior is separate from copying these files.

## Verification

```bash
python3 -B -m unittest discover -s tests -p 'test_package_*.py' -v
# Trusted local tar -> disposable private HOME/XDG/prefix only:
python3 -B tests/test_package_install.py --real-smoke
```

[Synthetic tests](../tests/test_package_install.py) cover preflight without writes,
new install, private ledger contents/hashes, empty override defaults, existing late
targets, source/destination symlinks (including source-root ancestors), unsafe
paths, wrong architecture, GLib quoting, collision rollback and replacement
preservation during cleanup. Uninstall tests cover hash/mode-verified full removal, mode-change refusal,
unrelated-file preservation, non-destructive refusal for a modified file, refusal
for a symlink swapped in, tampered/outside-prefix ledger paths, bad recorded hash,
missing ledger and `--uninstall --check` rejection.
Forbidden helper stubs must remain uncalled. The real smoke builds/extracts only
known files from a trusted local tar into its own temporary directory, installs
and then uninstalls under a private HOME, keeps an unrelated operator file, and
runs only `--version` before removal. It does not touch the real HOME,
launch the GUI, update caches or operate services. CI runs this alongside tar,
Debian, AppDir and native regressions. Real-user install/upgrade/uninstall,
desktop-cache/launcher and clean-distro/ABI acceptance remain pending.

## The setup and pairing screens

Both screens are static files in `ui/` loaded by the webview. CI renders them
with a real WebKit view and asserts the expected controls exist (install, cancel,
retry, server, quit; URL, close, refresh, QR) plus the pairing note about access
control, so a broken asset is caught without starting a desktop session.

### What the QR points at

When the forwarder is active the pairing window's QR and URL use the discovered LAN
address plus the proxy port (`http://192.168.1.50:3091/?token=…`) and the note counts
the remaining seconds. Nothing is advertised when the forwarder is off, expired,
revoked, bound only to loopback, or when no LAN address can be discovered — the
window then keeps the loopback-only explanation. The token is appended at render
time and is never stored in the proxy status.

### Page/command drift

A test also extracts every `invoke("…")` name from both pages and compares it with
the `tauri::generate_handler!` list in `src/main.rs`: a page that calls an
unregistered command fails CI instead of failing at runtime in the webview. The
check also refuses page calls to `restart_service`, which stays a tray action.

### Page/payload drift

Both pages read named JSON fields. A unit test extracts every `state.<field>` the
pages use, serializes the real `SetupState`/`RemoteState`, and fails when a field
the page reads is not sent (including the nested `server.baseUrl`,
`server.mode`, `runtime.version`, `runtime.node`). Renaming a Rust field without
updating the page is therefore a test failure, not a silently broken screen.

### What the pairing page may call

The pairing window renders whatever `get_remote_state` reports: with no pairing
URL the URL box stays hidden and the loopback-only note is shown, and after the
backend reports an exposed URL the **Refresh** action renders that URL and the QR
markup. **Close** calls `close_pairing`. Simply opening the window calls neither
`set_exposed_state` nor `close_pairing`.

### What the setup page may call

The page talks to the desktop client only through
`window.__TAURI_INTERNALS__.invoke(command)`. CI stubs that surface at
document-start and asserts the real script's contract: `get_setup_state` and
`prepare_service` run on load, nothing installs or cancels until asked,
`start_bootstrap` is called by **Install runtime** and keeps cancellation
reachable, `cancel_bootstrap` is called by **Cancel install**, and the auxiliary
buttons use `save_diagnostics`, `open_diagnostics_folder` and `quit_app`. Renaming
a command in Rust without updating the page therefore fails CI.

### Exported profile snapshots

`--export-backup` / `--import-export` work for configuration snapshots as well as
whole-data ones: the archive records scope `profile-config`, the import keeps the
`data/` layout the profile engine expects, and re-exporting the imported copy
reproduces the same bytes. The engine's own `--verify-profile-backup` accepts the
imported snapshot.

## Short-lived pairing proxy

The pairing window has a **Temporary LAN forwarder** card: *Forward 5/15/60 min*
starts the proxy through `start_pairing_proxy` (bounded 60-3600 s), the status line
shows the listening address, the loopback target, the countdown and the live
connection count, and *Stop forwarder* calls `stop_pairing_proxy`. The TTL always
ends it even if the window is closed. The same proxy is available from a terminal:

```
dsh-ocean --pairing-proxy --ttl-seconds 600 [--port 3091]
```

The proxy accepts LAN connections on `0.0.0.0:<port>` (default 3091) and forwards
them to the loopback server. It refuses a non-loopback target (nothing to hide),
caps the TTL at one hour, stops accepting and closes live connections when the TTL
expires or on Ctrl-C, caps concurrent connections and logs nothing — no request
lines, headers or tokens. `--check` binds an ephemeral loopback port, prints the
status JSON and exits, which is what CI exercises.

Limits: the proxy adds **no TLS and no authentication**; whoever reaches the port
still needs the Harness launch token, and the token is the only access control. It
does not make the server trust the proxy authority, does not rewrite the URL, and
ends when the client stops. The tray does not start it yet.

## Window-close preference and bootstrap version

In the client's configuration, `"closeToTray": true` (the default) hides the
primary window when closed. Set `"closeToTray": false` to quit Ocean when the
main or setup window is closed; this uses the normal shutdown path. Auxiliary
windows, such as pairing, still only hide and never quit the application. The
setting is read on each close. An independently managed systemd web service is
not stopped by quitting Ocean; an Ocean-owned child follows normal quit cleanup.

The legacy `dshVersion` configuration key does **not** select a bootstrap version.
The embedded runtime manifest pins that version. `--doctor` now reports the key
as unsupported/ignored instead of implying that the requested version is used.

## Client directory overrides

The client accepts only absolute `HOME`, `XDG_CONFIG_HOME`, `XDG_STATE_HOME`,
`XDG_DATA_HOME`, `XDG_RUNTIME_DIR`, `DSH_HOME` and `DSH_OCEAN_DATA_DIR` paths.
Empty or relative values are treated as unset, never as working-directory paths.
Missing/invalid `HOME` is resolved from the current user's account database;
there is no invented `/` fallback. XDG directories then use the usual home
subdirectories; `DSH_HOME` defaults to `~/.dsh`, and the private runtime data to
`~/.local/share/dsh-ocean` (or the valid XDG data override). If no valid XDG
runtime directory is set, session sockets/locks and restore checks consistently
use the client's state directory. Resolving these paths does not create them.
These rules describe the client, not external programs' environment handling.

## Plugins overview

The tray's **Plugins…** window reports the bundle names and dependency versions
declared by the web profile's `package.json` (names and versions only, read-only,
Close only). Installing or removing plugins stays DSH's job; the parser ignores
`scripts`, drops traversal-looking names and non-version strings, and shows no
paths or file contents.

## Exposing the UI to a phone on the same network

`dsh-ocean --pairing-plan` explains what is missing; `dsh-ocean
--prepare-exposure <lan-ip>` writes a candidate unit to
`$XDG_STATE_HOME/dsh-ocean/exposure/<unique-plan>/dsh-web.exposed.service`
with mode 0600 alongside an exact pre-change unit copy, and
prints the apply/verify/revoke commands. Applying it is a deliberate human
action (`systemctl --user daemon-reload && systemctl --user restart
dsh-web.service`); keep the pre-change unit so revoking is one copy away. Ocean
itself never restarts the service, edits the installed unit or opens a port. This
prepares a browser URL only; this command does not pair a device or establish
trust by itself.


## First DSH setup in Ocean 0.1.3

The setup window installs the complete publisher-built runtime for Linux x86_64
or ARM64. It requires HTTPS access to GitHub and system `curl`, but no preinstalled
Node.js, npm or pnpm. The bundled public runtime key authenticates the metadata;
release `0.2.0-rc.2.ocean.2`, channel `next`, sequence 2 and the reviewed architecture
hashes/components are pinned by this Ocean version. A downloaded public key is
never used as authority. The custom version is not requested from npm.

Before selection, Ocean imports and checks the complete archive and runs native
checks with isolated synthetic state. Cancellation remains available during this
preparation. The commit boundary rejects late cancellation; interruption during
commit leaves a journal that blocks normal starts and maintenance. Retrying setup
recovers only its own incomplete selection, retains accepted sequence floors and
checks the signed candidate again. An unknown selector or corrupt journal fails
closed and requires investigation. Existing runtimes and running servers are not
replaced by this first-install operation.

After installation, the normal server setup and native localization consent apply.
A server-start error does not mean the already verified runtime was removed.
Bootstrap trust is bundled solely for this exact installation and does not enroll
or replace operator updater settings. Shell remains `stable`; this DSH is `next`.

First installation uses an immutable bootstrap contract embedded in this Ocean
version: the existing runtime signature, exact metadata digest, release, sequence,
components and both archive hashes must all match. Its reviewed bytes remain
installable after the metadata's updater expiration date; this is an explicit
bootstrap policy, not an expiry-error fallback. Changing any metadata or payload
requires a newly reviewed Ocean release. GitHub assets must remain available.
Ordinary runtime updates still enforce signature freshness, the system clock,
sequence floors and manifest anti-equivocation. First installation never relaxes
those updater checks or changes operator trust settings.
