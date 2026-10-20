# Shell status and update guidance

The tray has distinct **Check for DSH core updates…** and **DSH Ocean shell
updates…** entries. Core checks concern npm/runtime generations. The shell
screen concerns this native client and does not trigger either kind of update.

```bash
dsh-ocean --shell-status
```

The CLI runs before GUI/runtime/service initialization and emits version,
installation **hint**, evidence, guidance and explicit availability flags.
It accepts no further arguments. There is no package-manager/network/profile/
service probe, external command execution, file write, restart or replacement.
Executable paths and APPIMAGE/FLATPAK_ID payloads are not included in the report.

## Evidence is not ownership

The classifier intentionally reports only conservative, non-authoritative hints:

- Nonempty `APPIMAGE` / `FLATPAK_ID` environment values are hints, not proof of
  this executable's origin. Their actual values are not read into output.
- A conventional `target/debug|release/dsh-ocean` path suggests a development
  build; it is not an authenticated installation record.
- `/usr/bin`, `/usr/local/bin` or `/opt` suggests a system prefix, **not** a
  package-owned executable. No pacman/dpkg/rpm database is queried.
- Conflicting inherited environment/path hints are shown as conflicting;
  other cases remain unknown. No install route is inferred as safe to apply.

Every report currently says `ownershipVerified:false`,
`automaticUpdateAvailable:false` and `productionSignedFeedConfigured:false`.
The screen gives conditional original-installer guidance for AppImage, Flatpak,
AUR/Arch, deb/rpm, development and unknown contexts. It never runs update
commands, opens a release/download URL or offers an apply button. Package
ownership and the actual installer must be confirmed independently. No managed
Flatpak/deb/rpm package or full engine/portability matrix is implied by a hint.

## Native screen

The GTK screen shows the compiled shell version, hint/evidence and limits as
plain selectable text. It has only a Close action. It explicitly separates
shell from core and says that no online version check was performed. The screen
is native Rust UI, not remote Tauri IPC or a change to the official DSH frontend.
Offline draft/sign/verify/stage tools are described in
[release verification](RELEASE_VERIFICATION.md); they do not make this screen
an updater or configure a production trust root.

[Tests](../tests/shell_status.rs) cover classification/conflicts, false-authority
flags, path/environment privacy and a headless CLI with fake package/service
commands that must not be called. A separate ignored GTK test checks text,
modal parenting, Close and the absence of Accept/Apply buttons, explicitly on
CI's private XDG/non-activating D-Bus + Xvfb display. Manual installed-client tray
click/visual acceptance and real package ownership detection remain pending.

```bash
cargo test --test shell_status
DSH_OCEAN_TEST_DIALOGS=1 GDK_BACKEND=x11 timeout --kill-after=5s 40s \
  bash scripts/test-native.sh xvfb-run -a cargo test --test shell_status \
  -- --ignored --test-threads=1
```
