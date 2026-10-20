# Desktop notifications

Desktop notifications are **enabled by default**, as requested. The native
tray's **Desktop notifications** checkbox turns them off/on and persists that
choice separately in `~/.config/dsh-ocean/notifications.json`
(`XDG_CONFIG_HOME` is respected). A previously saved `false` is not overridden
by an update. Missing preferences mean enabled; malformed, linked, oversized or
future-format files fail closed and are not silently replaced.

## Implemented scope

Native download completion and failure send generic desktop notices. Cancelled,
blocked and still-running downloads do not send one. Notification delivery is
asynchronous and cannot make a successful file save fail.

Task activity is connected through a narrowly-scoped observer of the official
UI's **existing** WebSocket event carrier, not a second session or a browser
Notification shim. It notices an observed running `true` → `false` transition
while Ocean's main window is unfocused. This means "stopped running", which can
include completion, cancellation, failure or another non-running state; no
successful-result claim is made. Initial/duplicate false values, disconnects,
logical stream endings and generation changes do not create completion notices.
No official frontend/core file is modified.

## Privacy and behavior

Payloads are constructed from a fixed enum, not arbitrary strings:

- `Download saved` / `Download not saved` with a request to open Ocean history.
- Task notices contain only `DSH task activity` and `Open DSH Ocean to view
  the update`, never a claimed successful result.

No task text/title, session/agent identifier, filename, file path, URL, cookie,
response body or raw provider error is included. The desktop service still
learns that Ocean has activity. Its own history/lock-screen policy is outside
Ocean's control; transient, quiet and five-second expiration hints are requests,
not delivery/retention guarantees.

The app rate-limits to three accepted notices per 30-second window and at most
one of each kind per ten seconds. Same-kind notices replace the previous owned
notice when possible. Disabling prevents new sends, clears queued notices and
tries to close owned ones, including replies arriving after disable. An
already-in-flight banner can briefly appear before the close request arrives.

The freedesktop D-Bus service is optional. Missing/unavailable services fail
quietly without a shell-command fallback. Clicking a recognized default action
only focuses the existing Ocean window; no URL, file or command from a
notification is executed. IDs are paired with the daemon's unique bus owner,
so replacing/restarting it cannot make Ocean close or activate another app's
reused notification ID.

Preference files are bounded to 8 KiB and created privately (`0600`, new parent
directories `0700`) using atomic replacement. Main Ocean configuration is not
rewritten. Failed persistence does not undo an explicit disable for the current
session; the native warning explains that restart may restore an older setting.

## Verification

Pure tests cover the enabled default, persistent disable, malformed/future/link
refusal, permissions, fixed D-Bus payload signatures and rate limits.

A separate fake daemon test must run inside a private D-Bus session. It refuses
to acquire the notification name on the user's ordinary `/run/user/...` bus:

```bash
timeout --kill-after=5s 45s bash scripts/test-native.sh \
  cargo test --test desktop_notifications \
  -- --ignored --test-threads=1 --nocapture
```

It tests delivery, unknown/owned actions, disabling and replacement-daemon ID
reuse without displaying real desktop notifications. The [native test harness](../scripts/test-native.sh)
uses private XDG runtime/config/cache/data/state directories and a D-Bus config
without activatable service directories, so GTK cannot autolaunch the user's
GVFS/accessibility/systemd helpers from the fixture bus. Display connectivity is
preserved separately; these isolation settings never affect the shipped app.
CI executes it explicitly. Manual acceptance on the user's actual notification daemon remains pending.

## Task adapter and compatibility limits

The inspected gateway's existing WebSocket carrier is base-relative
`api/remote.mux`. Its `$events` stream opens with
`{type:'open',streamId,endpoint:'$events',payload:{args:{}}}`, receives a strict
`{type:'ready',clientId,host:{home}}` value, then carries
`{type:'item',streamId,value:{type:'emit',event:'api-session/status',args:[agentId,running]}}`.
The observer passes original sends/return values/errors unchanged and leaves
the WebSocket constructor/statics/subclasses intact. No new socket, RPC,
subscription or session is created. It ignores foreign socket origins,
unrelated endpoints, binary/oversized frames and unknown shapes. State is
bounded to eight logical streams and 1024 opaque IDs per socket and is reset
on disconnect/generation replacement. IDs remain only in page memory.

The observer runs in the top-frame main world before page scripts. Only a
constant DOM event crosses to a top-frame isolated WebKit world, which exposes
a constant numeric native signal. The native message handler is not available
in the page world. Rust rechecks the current document's exact server origin,
accepts only the numeric sentinel, and applies preference/focus/rate policy.
A trusted page can spoof the anonymous DOM signal, so this is intentionally
only a rate-limited generic notice, never a privileged command or task URL.
No Tauri remote-IPC permissions are widened.

The contract was inspected in installed `0.2.0-rc.2` and public
[`0.2.1-rc.2` gateway source](https://unpkg.com/@deepseek-ai/dsh-api-gateway@0.2.1-rc.2/lib/types/stream-protocol.js),
[URL construction](https://unpkg.com/@deepseek-ai/dsh-api-gateway@0.2.1-rc.2/lib/types/client/stream-client.js)
and [status emission](https://unpkg.com/@deepseek-ai/dsh-api-session-controller@0.2.1-rc.2/lib/types/index.js).
It remains an implementation contract, **not a version-stable public API**.
Unknown versions with the same recognized schema may still work; other shapes
are ignored. In-process/cross-origin transport overrides and worker-owned
sockets are not observed. There is no initial running-state replay, so opening
Ocean mid-task, reconnecting, or state eviction can miss a transition. Delivery
is best effort and coalesced, not exactly once; the client must remain running.

Node protocol tests exercise transitions, transparent send semantics, state
bounds, malformed data, disconnects, readiness and late bridge installation.
A separate GTK test uses an ephemeral context, real loopback WebSockets and a
private fake notification daemon. It proves the full carrier → isolated world
→ Rust → D-Bus path, duplicate/initial suppression, origin rejection before
rate limiting, wrong-payload rejection, disabled delivery, opaque payloads and
exactly one fixture socket per document. No real DSH task/model request is run.

```bash
node --test tests/task_observer.mjs
timeout --kill-after=5s 60s bash scripts/test-native.sh \
  cargo test --test task_notifications -- --ignored --test-threads=1
```

CI maps the fixture on a virtual Xvfb display. Manual acceptance with a real
DSH task and the user's actual notification daemon, task deep links and a
broader engine/version matrix remain to be verified.
