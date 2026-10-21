# Shell updates and original-installer guidance

The tray has separate **Check for DSH core updates…** and **DSH Ocean shell
updates…** entries. The native shell screen supports explicit signed
check → download → apply, retained rollback and interrupted-selector recovery.
It never silently replaces the running client. Signed core orchestration remains
separate M2 work at this checkpoint.

## Evidence is not ownership

```bash
dsh-ocean --shell-status
```

This particular CLI remains a read-only hint classifier: no package database,
network, profile/service probe, command, write or restart. APPIMAGE/FLATPAK_ID
payloads and executable paths are not emitted. Its availability flags remain
false; it does not inspect an operator's configured trust or grant mutation
permission.

- APPIMAGE/Flatpak environment values do not prove the running image's origin.
- A development/system-prefix path does not prove package ownership.
- Conflicts remain unknown. No pacman/dpkg/rpm commands are run.
- Package users must update the **original** with its installer/package manager
  (AUR/deb/rpm/Flatpak as applicable). No complete package matrix is implied.

## Explicit managed destination

The native updater only acts in a separately opted-in, owned destination:
`$DSH_OCEAN_DATA_DIR/shell`, or normally
`$XDG_DATA_HOME/dsh-ocean/shell`. It must be separate from Harness home.

Enrollment creates an owned private purpose/version marker. That local record,
protected ancestry, known layout and verified retained payload receipts—not an
APPIMAGE/path hint—establish destination authority. Marker IDs are not remote
credentials or a same-UID security boundary. Unknown existing contents,
symlinks/writable/unowned state and incomplete un-enrolled installations are
refused, not adopted or erased. Existing operator-installed versioned prefixes
require valid retained receipts before enrollment.

The original executable and original desktop entry are never overwritten. A
successful managed install creates a **separate** `DSH Ocean (managed)` application
entry (`dev.dsh.ocean.managed.desktop`) whose Exec follows the managed `current`
selector. Desktop argument/value escaping is applied; unknown existing launcher
contents refuse overwrite. A launcher failure after a committed selection is
reported as a warning with the installed-on-disk state, not falsely as rollback.

```bash
dsh-ocean --managed-shell-status                    # read-only, no enrollment
dsh-ocean --enroll-shell --confirm-managed yes      # explicit separate destination
dsh-ocean --install-managed-shell --kind tar.gz --confirm-install yes
dsh-ocean --rollback-managed-shell --confirm-rollback yes
dsh-ocean --recover-managed-shell --confirm-recovery yes
```

Installation requires independently provisioned configured trust, authenticated
fresh metadata, exact verified bytes and a fresh settings guard spanning final
candidate revalidation through activation. See [trust and transaction policy](RELEASE_VERIFICATION.md).
The CLI install command explicitly installs the configured latest candidate;
GUI actions show it first and separate download from application. Normal managed
updates retain the installed format and refuse disk-version downgrades even if
an operator removed the remembered sequence file. Deliberate retained rollback
is a separate confirmed action and never lowers that floor.

## Native controls and lifecycle

The GTK dialog shows the compiled running version, selected-on-disk version,
operator trust fingerprints, shell/core `stable`/`next` preferences, format and
conditional original-installer guidance. Public keys are never discovered from a
feed or printed as key material. Production roots/endpoints are unconfigured by
default; the screen explains independent provisioning with the existing advanced
CLI, rather than offering TOFU. Saving a channel invalidates prior candidates and
does not change legacy npm/Harness configuration.

- Opening/refreshing reads local state only; network starts on explicit Check.
- Check is metadata-only; Download stages non-executable bytes; Apply requires
  an enrolled destination and explicit confirmation.
- Cancellation wins until the download/result publication boundary. After an
  explicit local Apply/Rollback/Recover starts committing, it is noncancellable.
- One native operation gate covers dialogs. Buttons disable synchronously on
  worker start, not on a later polling tick. Blocking work stays off GTK.
- A quit request cancels preparatory work or waits for a committing transaction
  and owned cleanup before app shutdown. No updater invoke commands/capabilities
  are added to the remote Harness webview.
- **Restart is explicit and simple:** quit this client, then open **DSH Ocean
  (managed)** from the application menu. Closing to tray is not quitting. No
  automatic process-replacement/singleton-handoff protocol is implied; shell
  actions do not restart the core or migrate a profile.

## Evidence and acceptance limits

[Classifier/native tests](../tests/shell_status.rs) cover hint privacy and a real
isolated GTK sequence: unconfigured refusal, channel save without enrollment,
explicit enrollment confirmation, fixture-root signed metadata check, verified
download, two confirmed applications and retained rollback without lowering the
floor. Fake trusted curl and fixed **test-only** keys are used; no live HTTP,
real publisher key, user profile or service participates. [Headless tests](../tests/update_feed_cli.rs)
also cover original-install preservation, unknown destination/launcher refusal,
bad body cleanup and disk-version downgrade refusal.

```bash
cargo test --locked --test shell_status --test update_feed_cli
DSH_OCEAN_TEST_DIALOGS=1 GDK_BACKEND=x11 \
  bash scripts/test-native.sh xvfb-run -a cargo test --locked --test shell_status \
  -- --ignored --test-threads=1
```

Installed-client tray/visual acceptance and the final second-laptop run remain
owner acceptance. These fixtures do not establish production trust, genuine
AppImage-runtime acceptance, package ownership detection or release clearance.
