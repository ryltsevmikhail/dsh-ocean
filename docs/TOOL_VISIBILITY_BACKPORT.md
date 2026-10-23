# Tool visibility CPU-loop backport (2026-10-23)

## Scope and cause

MacBook Air running Omarchy Linux; local host `omarchy`, not a remote macOS target.
DSH `0.2.0-rc.2`, generation `gen.aQwngq`, profile `web`, service `dsh-web.service`, localhost port 3080.
Installed profile package: `~/.dsh/profiles/web/node_modules/dsh-plugin-tool-management`, version `0.20.0`.
Browser runtime: `@deepseek-ai/dsh-experimental-browser-use-runtime` `0.2.0-rc.2`.
The loaded package paths were verified in CPU profiles and via V8 Debugger script source.

With `carrierHiddenTools` present, `applyToolRestrictions()` unconditionally released every agent restriction on each replay. Releasing erased the per-agent comparison keys. Reinstalling layers emitted `tools/change`, triggering another replay after 300 ms. Browser runtime `refreshBlockedMasks` rebuilt and deeply copied tool schemas on every layer change.

The backport keeps live agents' layers and comparison keys. `restrictAgent()` already replaces changed lists and releases empty lists; only orphan layers are removed during replay.
Equivalent upstream fix: https://github.com/ouli-1242/dsh-plugin-tool-management/blob/e42e470eb1cd86a8c4f6ad11e96e89ab26249cf3/src/mcp/tool-visibility.ts#L656-L669
No full plugin upgrade, browser/MCP disabling, session deletion or session-storage changes.

## Reproduce, verify, reinstall

From the repository root, using Node with the built-in test runner:

```sh
node scripts/backports/tool-visibility.cjs --check
node --test scripts/backports/tool-visibility.test.cjs scripts/backports/tool-visibility-installer.test.cjs
# Explicit opt-in after reinstalling the exact supported plugin:
node scripts/backports/tool-visibility.cjs --apply
```

Installer optionally accepts the package directory as its second argument. Tests accept `VISIBILITY_FILE` pointing to the installed module or original backup. The installer checks package name/version and the entire original SHA-256:

`a8aacdd24b069c2e10aaee82ea9cb57d2da1521b7759ac0c1805e1bbc5086b34`

Unknown versions/content are refused, not blindly patched. Already-patched content is verified by reconstructing and hashing the original. Applying creates an exclusive `.cpu-backport.original` backup, validates syntax, and does not restart DSH. This is a manual installation step, not an automatic package-manager hook. Reinstallation can overwrite the live patch; rerun `--check`. For a newer upstream version, review whether the fix is included and retire this backport rather than bypassing the version guard.

The unified patch is `scripts/backports/tool-visibility-0.20.0.patch`. Restriction tests use the actual module body in a VM with import-boundary stubs and deterministic timers. They verify self-event convergence, stable layers, changed/empty deny lists, distinct agent restrictions and stale carrier promises after replay/detach/dispose. Before the backport, 2 of the 5 restriction tests failed on non-convergence; afterwards all 5 passed. Installer tests cover apply/backup/idempotence/rollback and rejection of unknown content, versions and damaged backups. These are focused unit tests, not a full DSH integration suite.

## Observed activation and CPU

One restart of the existing systemd user service was explicitly approved. PID changed from 908 to 70035. V8 `Debugger.getScriptSource` confirmed original code before and patched code after activation. Temporary inspector listener on localhost:9229 was closed; port 3080 remained active.

| Metric | Before | After |
| --- | ---: | ---: |
| Process CPU, one core = 100% | 104.7% | 6.5% |
| CPU profile duration | 30.226 s | 30.177 s |
| Inclusive refreshBlockedMasks | 92.65% | 0% sampled |
| Inclusive snapshotJsonValue | 83.53% | 0% sampled |
| Profile idle | 0.02% | 98.11% |

CPU percent uses `/proc/PID/stat` deltas and wall time, not lifetime `ps` averages. The post-restart wall interval reported 29.050 s while the profiler monotonic interval was 30.177 s; wall-clock adjustment/timer behavior can affect the CPU-percent denominator. Treat percentages as approximate. Measurements used the same service/profile and enabled configuration, but a restart may change active session/agent state; this is not an identical-workload benchmark. The focused tests independently reproduce and eliminate the event loop.

The earlier supplied 30.257 s profile (SHA-256 `65bf7711136118f003f04458ba176d4fd966ab3aba795b9e16b7ae26f469ff42`) independently showed 84.99% inclusive refreshBlockedMasks, 77.89% snapshotJsonValue and 1.53% idle.

Raw CPU profiles remain local and are intentionally not committed: they may disclose local paths and execution details. No automatic restart/profiling helper is installed by this backport.

## Rollback

Installer-created backup:

```sh
node scripts/backports/tool-visibility.cjs --rollback
# Only after coordinating interruption of other sessions:
systemctl --user restart dsh-web.service
```

For the initial incident, the original backup is instead at:

`~/.dsh/profiles/web/node_modules/dsh-plugin-tool-management/lib/mcp/tool-visibility.js.pre-cpu-backport-20261023`

Verify its SHA-256 against the original above before copying it over `lib/mcp/tool-visibility.js`. Coordinate a service restart to activate rollback. Disk restoration alone does not replace already-loaded code.
