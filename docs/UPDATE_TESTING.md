# Runtime lifecycle and update validation

The opt-in [integration test](../tests/real_service_update.rs) exercises actual
npm runtime generations and the real user systemd manager. It is separate from
the fast mock/fault tests in [generation_update.rs](../tests/generation_update.rs).

## Run explicitly

Requires Linux, a working user systemd manager/journal, network access to Node
and npm, build dependencies for the runtime, and roughly 3 GiB of temporary
space. No existing DSH installation or credentials are required.

```bash
cargo test --test real_service_update \
  real_versions_activate_and_roll_back_on_an_isolated_service -- --ignored --nocapture
```

It is intentionally ignored by ordinary `cargo test`: the standard CI job
compiles it but does not have to provide a running user manager or network.
The clean-bootstrap workflow remains a separate no-system-Node test.

## Isolation contract

- A unique `dsh-ocean-test-*.service` is runtime-linked to a private temporary
  file. No production unit file or permanent user-unit configuration is edited.
- Test service and installer use `env -i`, an explicit private HOME, and pinned
  runtime tools. The service has private DSH/XDG directories and no access to
  ambient provider variables or the user's npm configuration through HOME.
- The service starts on port zero; only its invocation-scoped journal reveals
  the assigned loopback port. That port is then pinned for restarts. Port 3080
  is refused.
- The updater's test entry point requires the test-unit namespace and a HOME
  below the private prefix. The ordinary desktop entry point still only accepts
  `dsh-web.service`. A mismatching filename/namespace fails before manager calls.
- The health helper queries only the specified unit and current invocation;
  token-bearing URLs/journal messages are never printed.
- Workers/services have maximum runtimes. Cleanup stops only exact test/worker
  names created by this process, removes its runtime link and deletes its own
  private fixture. If shutdown cannot be confirmed, the fixture is retained.
- The existing production service PID is read before/after and must not change.

The narrow test entry point is a library helper, not a remotely callable GUI
command or a user-configurable production-unit override.

This is configuration/process isolation, not a filesystem sandbox against
malicious dependency code: processes still have normal user privileges. The
test performs no model requests or model-tool execution.

## What is exercised

1. Download/install the pinned Node and DSH `0.2.0-rc.2` without system Node,
   including native-module and isolated authenticated runtime health checks.
2. Start a real isolated service; verify its private environment and 401/303 auth.
3. Run the same detached updater worker used by the desktop client to install
   DSH `0.2.1-rc.2`, replace/reload/restart the test unit, and pass live health.
4. Force a fresh older-version candidate installation, pass its real 401/303
   health, then inject a rejection of that candidate's activation. The UI never offers this downgrade; it
   is only a controlled failure scenario for the transaction.
5. Prove the previously successful generation and exact unit bytes are restored,
   its current service passes auth, and an isolated data marker is unchanged.

The test does not validate the native confirmation/reload UI, signed feeds,
package publication or arbitrary profile/data migration compatibility. Runtime
and unit rollback is not a coordinated backup/rollback of shared session stores.
See [profile snapshot scope](PROFILE_BACKUPS.md).

## Real managed-child lifecycle (no user service or downloads)

The separate `real_managed_child` integration binary runs the production
`managed_child::start` helper against an explicitly supplied installed runtime.
It does not call bootstrap, npm, systemctl or any model/session API. The runtime
is reused as an executable input; no generation is installed or selected.

```bash
# Default: fixture-only watchdog regression; no installed runtime needed.
cargo test --locked --test real_managed_child
# Opt-in: choose the exact trusted generation, not a temporary-directory glob.
DSH_OCEAN_TEST_RUNTIME_ROOT=/absolute/path/to/generations/gen.EXAMPLE \
  cargo test --locked --test real_managed_child real_managed_child_lifecycle \
    -- --exact --ignored --test-threads=1 --nocapture
```

Use the exact test selection shown above: another ignored entry is an internal
watchdog worker, not a standalone acceptance case.

The controller exclusively creates a private temporary tree, then re-executes a
worker with an allowlisted environment and empty private working directory.
HOME, DSH_HOME, XDG config/data/state/cache/runtime and temporary paths point
inside that tree; provider variables and the user's desktop bus are not inherited.
Both launches use `--profile web --host 127.0.0.1 --port 0 --no-open`. A reported
URL must be a plain loopback root origin with a nonzero port other than 3080 before
any probe is sent. Tests require bare 401, authenticated 303, a fresh token on
relaunch and rejection of the previous token at the new server. No token-bearing
child log or token assertion value is printed. A private marker survives restart.

The Linux controller is a child subreaper: the DSH child owns a separate process
group, so killing only the worker group is insufficient. Controller tests are
serialized in a dedicated integration executable with no unrelated subprocess
work: adoption/reaping is process-wide, not a host-wide process scan. A bounded watchdog stops
the worker and kills/reaps adopted child processes before removing the fixture.
Fixture-only tests exercise orphan cleanup even before a readiness PID has been
published. Failed or unconfirmed cleanup retains the fixture rather than deleting
files still potentially in use. Force-killing the controller itself bypasses RAII;
this is not a cgroup lifetime guarantee or a filesystem/network sandbox against
malicious runtime code, which still runs with normal user privileges.

This covers real process startup/authentication/stop/relaunch through the extracted
helper, not Tauri's systemd-to-child fallback dispatch, health-monitor recovery,
window reload/cookies, task execution or provider compatibility. Those acceptance
gates remain separate. Ordinary CI runs the fixture-only cleanup regression and
compiles the opt-in case; it need not install/download DSH to run default tests.

Local acceptance on 2026-10-20 passed three consecutive real-runtime runs (six
actual Node/DSH launches) against one explicitly selected installed generation.
Each run verified private effective child environment/cwd, authentication and
rotation, marker retention, and that normal stop/relaunch needed no orphan cleanup.
The fixture-only watchdog test, full default Rust suite, formatting and strict
locked all-target clippy passed. The existing `dsh-web.service` MainPID/invocation
and installed Ocean PID were unchanged before/after. This is single-host evidence,
not a runtime/version/distro matrix or installed-window acceptance.

## CI GUI fallback and automatic recovery (controlled child fixture)

The ordinary `ci` workflow now includes a private Xvfb step using the **real Ocean
binary and WebKit**, but a controlled Python HTTP child instead of Node/DSH.
This is deterministic client/GUI state-machine coverage, not acceptance of the
real DSH HTML, provider compatibility or bootstrap installation. It needs no DSH
download, profile credentials, model requests or access to the running server.

The [controller](../scripts/smoke-child-recovery.py) requires the private native
bus/runtime and xvfb-run authority before launching anything. It clears the app's
environment, creates exclusive HOME/DSH_HOME/XDG/cwd directories, supplies only
loopback ephemeral ports (never 3080), and makes systemd unavailable through a
stub. The configured origin is a controller-owned 503 credential trap: it must
receive no token, cookie or Authorization header. All other service/journal/npm/
external-open helper invocations are forbidden. The real user unit is never used.
Software rendering and WebKit sandbox disabling are scoped to this private test;
they are not production settings or evidence for normal sandbox compatibility.

The [child fixture](../scripts/gui-child-fixture.py) implements 401/303/cookie auth.
Its page sends a generation-specific POST only after a visible animation frame;
the controller never sends that POST. Idle speculative browser connections are
bounded and served independently, rather than blocking health or render probes.
The smoke waits for the first authenticated frame and one visible native window,
kills only the server leader, then lets the production two-failure health monitor
recover without accelerated test hooks. It requires a second visible-frame beacon,
the same main-window ID and preserved data marker. A same-group sleep sentinel
must not survive recovery. Finally a real WM_DELETE_WINDOW with closeToTray=false
must exit Ocean successfully and leave no live app/server/sentinel descendants.
No success-path cleanup kill is allowed to mask a failed production shutdown.

Failure cleanup uses a dedicated Linux subreaper and signals only owned child
leaders/groups or individually adopted children. It scans owned task child lists,
not host process tables. The controller has a 110-second deadline; CI wraps the
whole private fixture in a 150-second timeout with cleanup grace. Private app
logs, profiles, cookies and token-bearing readiness lines are **not** printed or
uploaded. As with other process fixtures, force-killing the controller bypasses
its cleanup; this is not a cgroup or malicious-runtime sandbox.

From a source checkout with Xvfb, dbus-x11 and x11-utils available:

```bash
cargo build --locked --bin dsh-ocean
DSH_OCEAN_TEST_XVFB=1 timeout --kill-after=15s 150s \
  bash scripts/test-native.sh xvfb-run -a \
  python3 -B scripts/smoke-child-recovery.py "$PWD/target/debug/dsh-ocean"
```

Local validation passed: full default locked Rust tests, strict all-target clippy,
formatting, 62 Python tests (including 17 new fixture/controller tests), 24 Node
tests and CI YAML/step wiring checks. The owned-group regression also injects a
post-readiness worker failure to exercise subreaper cleanup before reporting it.
**Past internal hosted native acceptance (2026-10-20):**
Both `msrv` and `test` jobs passed during private development, including this GUI
scenario, real native launch, GTK dialogs, WebKit downloads/cancellation,
release/CLI and package smoke steps. The runner was Ubuntu 24.04 x86_64 with Xvfb.
The later generation-bound beacon also passed internally. These historical
results are not public run evidence and do not constitute a desktop/distro
matrix or full real-DSH UI acceptance. Check current public CI against the
published head and final conclusion.

The render beacon is additionally bound to the generation encoded in the HTML,
with no-store responses: a stale first-generation page cannot satisfy a later
render even if an ephemeral port is reused. Headless regressions reject that stale
POST before accepting the current-generation page. This tightens test fidelity;
it does not add production IPC hooks or alter the shipped client.

To rerun after pushing the intended branch, use its pull request CI or dispatch:

```bash
gh workflow run ci.yml --repo PUBLIC_OWNER/PUBLIC_REPOSITORY --ref YOUR_PUSHED_BRANCH
```

Dispatch requires the workflow to exist on the repository's default branch and
the tested branch/commit to have been pushed. Confirm the run's head SHA and final
conclusion before reporting success. Fork contributors should use their PR checks
or request a maintainer-run workflow rather than asking for credentials.
