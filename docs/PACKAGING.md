# Native packaging and explicit AppImage tool trust

The portable **binary** tar is built by [package.sh](../scripts/package.sh) and
its [Python helper](../scripts/package-tar.py). Python **3.11+** is a build-helper
requirement, not a runtime dependency of the installed Rust client. The portable
[install-only helper](INSTALLATION.md) now also needs Python 3.11+ at installation
time; it refuses overwrites/upgrades and runs no cache/service helpers.
Cargo/toolchain and
native GTK/WebKit development libraries must already be available. No installer,
service restart or package-manager install is performed by these builders.
[package-deb.sh](../scripts/package-deb.sh) and its [Debian writer](../scripts/package-deb.py)
reuse the same validated Cargo/input descriptors and no-overwrite publisher,
rendering Debian ar directly—not extracting a tar or reusing a staging tree.
No external `ar`, `tar` or `dpkg-deb` is required to build either format.

```bash
# Refuses an existing archive; never silently replaces it.
scripts/package.sh
# Choose another output directory for another build (parent must exist).
scripts/package.sh --output-dir /absolute/path/to/new-output
# Default epoch is 0; optionally choose an explicit unsigned 32-bit timestamp.
SOURCE_DATE_EPOCH=1700000000 scripts/package.sh --output-dir /absolute/new-output
# Same freshness/no-overwrite rules for Debian; no installation occurs.
scripts/package-deb.sh --output-dir /absolute/path/to/new-deb-output
```

## Freshness and scope

The helper always invokes `cargo build --release --locked --bin dsh-ocean` with
JSON diagnostics/artifact output and the explicit root manifest. It selects
exactly the root client's reported executable, **not** an existing guessed
`target/release/dsh-ocean`. Cargo's normal freshness decisions remain in effect;
`CARGO_TARGET_DIR` and configured output locations are respected. An unsuccessful
build, absent/invalid artifact stream or mismatched native ELF header/architecture
fails without publishing. Supported packaging hosts are Linux x86_64/aarch64.
A matching ELF header is only sanity checking, not loader/ABI compatibility proof.
The root Cargo manifest is bounded/read through a no-follow descriptor and
rechecked after build and before publication to reject detected version changes.
CI's earlier Cargo clippy/test/release commands also use `--locked`, rather than
silently repairing a stale committed lock before the packaging check.

Only explicit inputs are included: the executable, README/LICENSE/NOTICE,
installer/desktop file, five icon sizes and the named user documentation list.
No source/profile/session/global-credential stores, `.git`, runtime dependencies
or arbitrary/untracked directories are recursively swept into the archive.
Repository input directories/files are opened with held no-follow descriptors;
special files, final symlinks and detected changes fail. Regular inputs are
bounded to 16 MiB each; the executable to 512 MiB. Cargo binaries may have normal
Cargo hardlinks; packaging is not a general secret scanner for trusted repo files.

## Debian metadata and payload (no installation)

The Debian 2.0 ar has exactly `debian-binary`, `control.tar.gz`, `data.tar.gz`,
with normalized headers/order/padding and gzip/tar metadata. x86_64 maps to amd64,
aarch64 to arm64. Cargo SemVer prerelease `-` maps to Debian `~`; explicit package
revision `-1` preserves further upstream/build hyphens unambiguously. For example,
`1.2.3-rc-linux+build-2` becomes `1.2.3~rc-linux+build-2-1`, ordered before
`1.2.3-1`. This changes the old unrevisioned `.deb` filename/version policy.

Data contains `/usr/bin/dsh-ocean`, the absolute-Exec desktop entry, five hicolor
icons and user documentation under `/usr/share/doc/dsh-ocean`. `copyright`
concatenates the existing project LICENSE/NOTICE; this is not a complete dependency
license audit. Internal payload paths are normalized relative `usr/...`, never
archive extraction targets chosen by a user. Modes are 0755 for directories/bin,
0644 for other files; owners/times are normalized. `md5sums` covers every actual
payload file, including transformed desktop and generated copyright bytes. MD5
is only dpkg's local integrity convention, **not a signature/trust authority**.

There are **no maintainer scripts**, service units/actions or portable `install.sh`
in this payload. The builder does not invoke dpkg/apt, execute installer hooks,
read real DSH profiles or modify service state. Fixed dependency names describe a
system-GTK/WebKit host-native build; they are not ELF-derived/versioned ABI minima
or proof it installs/runs on arbitrary Debian/Ubuntu releases. Installed-Size is
an estimate from file bytes, not a target filesystem quota measurement. Control/
data intermediates are anonymous/private files, not reusable `dist/deb` staging.
Safe apply/rollback, actual package installation/upgrade/uninstall and clean
Debian/Ubuntu/arm64 desktop acceptance remain pending.

## AppImage tool gate and acceptance boundary

[AppImage builder](../scripts/package-appimage.py) now uses the same fresh locked
Cargo/input-FD/no-overwrite backend and a private, newly created AppDir. It never
reuses or recursively deletes old `dist/appimage` staging. There is no implicit
cached executable, mutable `continuous` trust or built-in production tool digest.
Both `--tool` and `--tool-sha256` are mandatory; missing/mismatched trust fails
before Cargo or tool execution. Select the correct native tool and independently
verify its exact bytes from trusted release material yourself. Hashing an opaque
download and treating that self-computed value as authority is not independent
publisher authentication.

```bash
# Explicit optional acquisition; no automatic fetch occurs during a build.
scripts/fetch-appimagetool.sh \
  --url "${PINNED_TOOL_URL:?choose a trusted HTTPS release URL}" \
  --sha256 "${INDEPENDENT_TOOL_SHA256:?verify independently}" \
  --output-dir /absolute/private-tool-cache
# Use the printed digest-named file; cached bytes are never chmodded/executed.
scripts/package-appimage.sh --tool /absolute/path/to/verified-cached-tool.AppImage \
  --tool-sha256 "${INDEPENDENT_TOOL_SHA256:?verify independently}" \
  --output-dir /absolute/path/to/new-image-output
```

[Acquisition helper](../scripts/appimagetool.py) validates the initial HTTPS URL
and every redirect **before following it**, with normal TLS validation. Plain HTTP,
userinfo URLs and downgrade hops are rejected. At most 256 MiB is streamed into
private scratch, SHA-256 checked before publication, then atomically linked to a
new digest-named cache file without replacing old/symlink/concurrent output. Cache
is 0600, non-executable. This is operator-selected pinning, not a production trust
root, signature, revocation policy or trusted upstream key configuration.

A bounded, owned, singly linked, non-group/world-writable regular tool is copied
through a no-follow FD to a private snapshot and checked for detected changes.
Only the verified private copy becomes executable. Its extraction/build runs in
private workspace with contained AppRun selection, not against the original cache.
**The selected tool executes arbitrary code with your privileges, not in a
sandbox**; it may run subprocesses/network, fetch a separate runtime or otherwise
act as that user. Pinning the tool does not pin all of its own downloads/dependencies.
Trust its behavior, toolchain and filesystem; this is not malicious-same-UID protection.
The project helper intentionally performs no installation/service operation.

AppDir includes fixed client/docs/icons/desktop inputs, generated argument-
forwarding AppRun, normalized modes/mtime and no portable installer. The produced
file must pass bounded regular/native ELF/type-2 AppImage **header sanity** before
shared private 0600 no-overwrite publication. Header checking is not squashfs,
loader, full format or ABI validation. Final 0600 images deliberately need an
explicit permission change before manual execution; no chmod/launch is performed
automatically. SOURCE_DATE_EPOCH is passed to the trusted tool; actual output
reproducibility also depends on that tool/runtime/compressor and is not asserted.

[AppImage tests](../tests/test_package_appimage.py) use only a project-generated
trusted stub and in-memory HTTPS responses. Its output explicitly says
SYNTHETIC-NOT-SQUASHFS: this is **not genuine AppImage acceptance**. They check
missing/wrong pin, unsafe source, private snapshot execution, fresh payload,
extraction/build/header/symlink failures, old staging/output preservation,
collisions, redirect-before-follow policy, cache size/digest/publication and no
real network. No independent real tool pin has been selected/executed in these
fixtures. A separate real native AppDir/AppRun `--version` smoke uses private HOME
and a spaced path without any external tool/image build; it proves staging and
headless argument forwarding only. Genuine-tool build, squashfs inspection,
manual launch/install and distro/arm64/engine acceptance remain pending;
no signed release is implied.

## Archive publication and reproducibility

Scratch is an exclusive random **file** with its original inode FD pinned through
publication/cleanup, not a reused staging directory. The
final output directory must be owned by the current user, not conventionally
group/world-writable, and not itself a symlink. Its descriptor/identity is held
and rechecked. User-chosen output ancestor aliases and Cargo output ancestor
aliases are not a sandbox boundary; trust your workspace/toolchain/filesystem.

After archive flush/fsync, same-directory hardlink publication refuses any
existing file, dangling symlink, directory or concurrent output collision. There
is no overwrite/unsafe-rename fallback. Only the identity-checked owned scratch
inode is unlinked; old outputs/directories are never removed. The final archive
is private mode **0600**, non-executable, then the output directory is fsynced.
Hardlink/IO/sync failures return failure; if failure occurs after publication,
an output may remain and durability is not claimed. Inspect it explicitly—do not
assume rollback or automatic deletion. Same-UID adversarial races and dishonest
filesystems are not prevented by these checks.

For tar/Debian, member order, modes, UID/GID/names and tar/gzip timestamps are normalized; gzip
contains no scratch filename. Identical input bytes/epoch with the same compressor
produce identical archives despite source mtimes. **This does not make Rust builds
reproducible**, pin a toolchain/source commit, attest provenance, sign a release,
prove publisher trust or configure a production update feed. Cargo may fetch
locked dependencies and execute trusted build scripts; this is not a sandbox or
an offline guarantee. Native libraries are not bundled; wider distro/engine/ABI
acceptance is still pending. Verify external releases with independent trust
material before extracting/installing; a local filename/checksum is not authority.

## Verification and remaining formats

```bash
python3 -B -m unittest discover -s tests -p 'test_package_*.py' -v
# Actual tar/deb + extracted --version only, private home, no installation:
python3 -B tests/test_package_tar.py --real-smoke
python3 -B tests/test_package_deb.py --real-smoke
# Real AppDir wrapper only, no third-party tool or genuine AppImage:
python3 -B tests/test_package_appimage.py --real-appdir-smoke
# Trusted local tar -> disposable private HOME/prefix install only:
python3 -B tests/test_package_install.py --real-smoke
```

[Tests](../tests/test_package_tar.py) use synthetic repos/fake Cargo output for
stale-default/custom-artifact selection, deterministic archive bytes, final and
parent symlink refusal, special/oversized inputs, manifest changes, permissions,
failed/missing/invalid/wrong-architecture binaries, existing/racing output
preservation and owned scratch cleanup. They do not simulate every directory/
scratch replacement, disk-full/power-loss or malicious same-UID/filesystem race.
The real smoke checks the allowlist and headless version without initializing
profiles/runtime or executing the installer. CI runs both plus existing native
regressions; no real DSH data, keys, sessions or desktop dialogs enter packaging.

[Debian tests](../tests/test_package_deb.py) independently parse ar/control/data,
verify all checksums/modes/times/gzip headers and no hooks/installer, retain old
staging/output, test prerelease/native architecture metadata and deterministic
bytes/collisions/failures. Real smoke inspects standard `ar`/`dpkg-deb` metadata
when available (CI requires both), then runs only extracted `--version` with a
private HOME. No package is installed and no maintainer hook runs. These checks
are archive acceptance, not a clean-distro install or runtime/ABI matrix.

All three builders share fresh Cargo inputs and no-overwrite final publication.
AppImage additionally has an explicit operator tool pin, but only synthetic tool
acceptance so far; none of these packages becomes self-contained by this change.
Signing/trust roots, genuine AppImage acceptance, release publication, safe shell
apply/rollback and installed-client acceptance remain separate roadmap items.
