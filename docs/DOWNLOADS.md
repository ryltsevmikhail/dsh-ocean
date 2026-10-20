# Native downloads

The main DSH webview uses WebKit's authenticated download session, a GTK Save
chooser, and **Downloads…** in the native tray. Nothing is refetched by a separate
HTTP client, and a finished file is never opened or executed automatically.

## Consent and origin policy

Each accepted request asks for a local destination. Existing files, directories
and links are deliberately not overwritten. If an existing name is selected,
the same chooser displays a warning and remains open for another name; it does
not discard/refetch the authenticated response. The backend independently
rechecks the destination and publication race. Cancelling the chooser cancels
that download without poisoning later requests. The dialog title contains only
the source origin, never URL user-info, paths or query parameters.

Native saving accepts the currently attached server's exact HTTP/HTTPS origin
and blobs from that origin. Request and response URIs are checked. Other
origins, opaque blobs, credential-bearing URLs, `file:`, `data:` and scripting
schemes are rejected. External downloads can be handled in a normal browser.
This is a save policy, not a network firewall or protection from compromised
same-origin web content; a response/redirect may already have been requested
when WebKit emits its download signals.

The main navigation policy also no longer treats every loopback port as the
same application. Only the attached origin stays inside the authenticated
webview. Other local applications and same-host scheme/port changes are
rejected rather than sent to another cookie jar. Ordinary external web links
are delegated to the browser. Local setup content does not gain new remote IPC
commands or a download-to-disk bypass.

## File safety

- Bytes first go into a uniquely-created private `0700` staging directory beside
  the user-selected destination, not into the selected file.
- Only regular, single-linked payloads can publish. The completed file becomes
  `0600`, without executable bits.
- Linux `renameat2(RENAME_NOREPLACE)` atomically publishes the payload. A file,
  directory or symlink that appears at the destination during transfer is never
  overwritten. There is no unsafe overwrite fallback.
- Detected destination-parent replacement stops publication. If publication
  fails, an available private staging copy is retained and its recovery path
  appears in history. Inspect it before reuse, especially after external
  directory changes; it is not a content-integrity or resumability guarantee.
- Failed/cancelled partial transfers clean only their own staging directory;
  they never remove a selected destination or an unrelated user file.
- Directory initialization is guarded before opening: `EMFILE`/open/validation
  failures remove only the known empty staging inode. The opened descriptor
  and current pathname must match its captured identity. Changed paths or
  unexpected contents are left alone. If identity cannot be read or cleanup
  fails, safe retention is preferred over an unverified delete.

The current limits are four active requests, one save dialog at a time and
1 GiB per file. Header, received-byte and observed payload-size checks cancel
oversized transfers; final-file size is checked again before publication.
This is not a disk quota against a hostile server. Rename/fsync/permission
failures can still prevent saving. No power-loss guarantee is claimed.

## History and interruption

History is bounded to 100 entries in the private state file
`~/.local/state/dsh-ocean/downloads.json` (`XDG_STATE_HOME` is respected).
It records status, origin, selected filename/path, and a recovery path when
needed. It does not record URL paths, queries, fragments, user-info, cookies,
response bodies or raw WebKit errors. Metadata may still contain private
filenames; it is not encrypted or automatically uploaded.

An unfinished record becomes `Interrupted` after restart. Inspect its selected
path before retrying: the file may have published immediately before a crash.
Private staging can remain after abrupt process termination. The native history
window refreshes current statuses in place (every 250 ms while open), including
background completion. Entries show creation time, newest first. Text remains
plain/selectable and control characters are escaped rather than interpreted.
Refresh sources hold only weak widget references and stop after closing.

**Live byte progress** shows received bytes and, when positive/bounded, the
server-reported content length. These are separate hints: content encoding,
missing/dishonest headers and unknown blob totals can make the counts differ.

The window also shows one **aggregate** progress bar for all currently receiving
transfers plus a count-only label. The bar is determinate only when *every* active
row reported a plausible total (positive, bounded, not below its received bytes);
otherwise it is an indeterminate pulse and the label says the total is unknown.
Finished rows never contribute. A **received** counter reaching a reported total
is still not a 100%-complete claim: the bar is a hint, network completion does not
mean publication, and only the existing successful no-overwrite publication path
produces `Saved`. Receiving rows say **not saved until Saved**.

Counters are monotonic/bounded in memory (at most the 1 GiB limit plus one
oversize sentinel byte), published to history at most every 250 ms per active
transfer, with final counters supplied by the terminal update. There is no
per-chunk disk persistence/notification or extra polling source. Terminal/unknown
rows cannot be created/revived by progress callbacks, and the engine rejects
callbacks after terminal processing or cancellation. The GTK view's existing
250 ms refresh renders changed counters without reopening.

Progress fields are `serde(skip)` volatile session data: even explicit history
persistence does not change the v1 disk schema. Counters reset to zero/unknown
after restart, including Saved/Interrupted rows; this is not durable byte
accounting, resumability, file-size validation or quota enforcement.

**Clear finished history** keeps the window open and retains active receiving
rows. It removes metadata only, never saved/recovery files. A persistence error
is shown explicitly: memory was cleared but prior entries may return after
restart. No success/durable-deletion claim is made when saving failed.
**Cancel selected transfer** uses an explicit Active transfer selector, with no
automatic default selection. Its model contains up to four current Receiving
IDs, not filenames-as-identity. Plain labels show escaped/bounded name and date;
control/bidi formatting characters are displayed as escapes, not used to reorder
visible names. Same-name transfers retain distinct IDs. Byte-only refreshes do
not rebuild the selector; metadata/status changes preserve selection by ID or
clear it if that ID is no longer active. No other row is substituted.

Response handling rechecks current Receiving status under the history mutex,
releases that mutex, and sends at most the one selected ID through the existing
native cancellation controller. A stale selection submits nothing. The selected
action is disabled without a controller/current selection. This adds no backend
URL/refetch/file-delete authority; the engine still excludes already-publishing,
saved, unknown and foreign-context IDs. Clear/Close never submit a cancellation.

**Cancel active transfers** remains the separate all-current action and requests
cancellation for a snapshot of the current
Receiving rows (at most four), not future downloads. A native-only GLib channel
coalesces requests to at most one queued message. Only exact IDs owned by the
current WebKit context can resolve to a native Download object; unknown/stale
IDs do nothing. The map borrow is released before calling WebKit cancel, since
terminal callbacks can run reentrantly. GTK/native objects never cross threads.
There is no remote IPC, URL/refetch/cookie argument or network retry.

The button reports **requested**, not guaranteed cancellation. Receiving rows
can already be finishing/publishing: those operations have left the native
active map and are not cancelled. Saved/publishing files are never removed.
Terminal cancellation stays `Cancelled` even when WebKit also emits `failed`;
only owned partial staging is cleaned. It does not emit a failed/saved desktop
notice. A save chooser already open may still need closing, but a cancellation
status recheck prevents staging/publishing the subsequently chosen path.

The button is disabled when no cancellation controller/Receiving row is present.
History clearing and dialog Close do not request cancellation. Automatic staging
recovery, retention/garbage collection and pause/resume remain unimplemented; the
progress bar is display-only and offers no resume or throttle control.

## Verification

[Progress tests](../tests/download_progress.rs) cover monotonic/bounded counters,
no implicit file writes or v1 schema change even on explicit persist/reload,
all terminal/unknown/cleared row guards and received-vs-reported rendering without
percent/Saved inference. A deterministic engine clock test covers throttling;
GTK checks live byte text refresh without persistence and that the aggregate bar is
determinate only with complete totals, and the real WebKit slow
response checks partial counters/total before cancellation plus exact counters
after the next successful save. Direct duplicate-finished/late-data signal
injection during publication is not driven; scoped static review found no
concrete lifecycle regression, not a comprehensive audit.

The [pure tests](../tests/downloads.rs) cover origin boundaries, traversal-like
suggestions, private/no-overwrite publication, racing destinations, parent
replacement, cancellation, limits, interruption and history privacy.
Transfer initialization unit tests additionally inject open exhaustion, an
unexpected descriptor, a replaced path and nonempty staging, without changing
real descriptor limits or exposing a production fault-injection hook.

The [real WebKit test](../tests/webkit_downloads.rs) uses a hidden ephemeral
context and a local fixture server, not the real DSH service. It requires a
synthetic HttpOnly session cookie, then exercises chooser cancellation, success,
rejected overwrite, later success and a blob export from an actual document.
It additionally cancels a slow authenticated response after bytes/staging exist,
checks partial cleanup/Cancelled terminal status, rejects a duplicate queued
request, and proves stale/saved IDs do not poison a later saved transfer. A
unit test covers invalid/oversized/coalesced/disconnected cancellation requests.
A separate [gated multi-stream WebKit test](../tests/webkit_multi_downloads.rs)
creates two ephemeral contexts with independent synthetic HttpOnly sessions and
three overlapping incomplete responses. The server sends a prefix but holds EOF
behind an explicit gate, rather than relying on timing to keep transfers active.
Cross-context ID requests are ignored. The real GTK selector cancels one owned
ID while the other same-context and foreign-context transfers remain Receiving.
Releasing EOF lets both publish their exact bytes with mode 0600; a later
same-context transfer also survives a stale selected ID. The fixture checks
single terminal events per ID, no fallback selection, no partial cancellation
destination, staging cleanup, unrelated-file preservation and cookie/query
privacy. It is bounded loopback/test-only infrastructure, not a production
injection hook, network benchmark or full engine matrix.

That fixture also opens the actual GTK Save chooser inside the native
`decide-destination` callback. Its test driver enqueues exact-ID cancellation
while `dialog.run()` is in its nested main loop, then uses a lower-priority GLib
idle fence to ensure the cancellation handler ran before clicking the real Save
button. A valid new path is returned **after cancellation**: the engine must
ignore it, leave destination/recovery unset, create no final/staging file and
record one Cancelled terminal event. A subsequent transfer must still save.
WebKit may defer failed/finished emission until destination handling returns;
the driver deliberately does not wait for terminal history inside the chooser.
A bounded watchdog closes the fixture dialog on failure. No test-driver cancellation or
auto-accept hook is compiled into the production chooser.

During past internal validation, the new scenario first exposed a SIGSEGV.
An isolated internal gdb reproduction localized the crashing test thread to `webkit_download_cancel` called by Ocean's
post-chooser guard, after queued cancellation → priority fence → late Save →
chooser return. That guard attempted a second native cancel after cancellation/
terminal work had run in the nested loop. Rust frames and the exported native
entrypoint were resolved; internal WebKit frames were not fully symbolized, so
no exact allocation/use-after-free mechanism is claimed. The CI memory snapshot
had 6.8 GiB available and the failure was SIGSEGV, not an observed OOM kill.

State-owned cancellation now uses a one-shot latch **set before entering C**,
refuses terminal/repeat calls and does not call native cancel after a failed or
finished chooser return. All state-owned cancellation paths share this gate;
the two pre-state rejected-view exits still cancel their newly rejected object
once. Late selected paths are still ignored. A pure unit test covers latch
reentrancy/terminal semantics and the unchanged native regression remains a CI
gate. The same fixture exited normally under gdb after the fix internally,
including the late real Save and subsequent successful transfer. Normal CI
repeats it without a debugger in fresh private XDG/D-Bus/Xvfb processes and does
not treat a failed first run as success. Diagnostics ran only on synthetic CI
fixtures with core dumps disabled;
no real user process/core was read, copied or uploaded. This does not establish
safety for every WebKit build or arbitrary external cancellation pattern.
These are past internal results, not public run links. Check current public CI
against the exact published head and final conclusion.

Choosers are injected into the network tests; no real download folder is
touched. A separate [actual GTK chooser test](../tests/download_chooser.rs)
exercises real Save/Cancel buttons and the nested dialog loop, checks modal
parenting/local-only selection/title privacy, round-trips a Unicode filename,
and retries an existing name in the same chooser before private publication.
It asserts that cancellation creates no file and existing bytes stay intact.
The test has no production auto-accept hook: its driver is in the test binary.
Its dialog assertions passed on isolated Broadway locally; CI uses Xvfb as
the authoritative display check, never the user's display. GTK `3.24.52-2`'s
Broadway daemon also hit a teardown SIGABRT once after the successful test:
`client_disconnected → client_free` asserted `disconnect_idle == 0` while a
GIO callback was closing the fixture connection. Other threads were waiting,
with no OOM evidence. This is a separate display-daemon failure, not a passing
Broadway teardown or an Ocean-client crash; the precise underlying race is not
proven. No production workaround or desktop reconfiguration is applied.
A [live history GTK test](../tests/download_history.rs) checks status changes
without reopening, clear/close buttons, retention of active rows, persistence
failure warnings, cancel-all/selected callback filtering/enablement, no default
selection, stable-ID selection across metadata refresh, stale terminal selection
rejection, nonselected Receiving retention and preservation of saved/recovery
files. Pure selector tests cover duplicate filenames, bounded escaped labels,
terminal exclusion and the four-row limit. Its callback is a fixture; actual
selected cancellation with overlapping streams and two-context isolation is
covered separately by the gated WebKit fixture, including cancellation inside
the actual nested Save chooser. In-progress publication races are not directly
driven in tests; scoped read-only
review found no concrete cancellation defect, not a full audit. It runs only on an
explicitly isolated display in CI; pure rendering tests run without GTK init.
Manual mouse/keyboard/visual acceptance on the installed client remains pending.

```bash
# Actual Save dialog, only on an explicitly isolated virtual display:
DSH_OCEAN_TEST_DIALOGS=1 GDK_BACKEND=x11 timeout --kill-after=5s 40s \
  bash scripts/test-native.sh xvfb-run -a cargo test --test download_chooser --test download_history \
  -- --ignored --test-threads=1

# Network tests only on an isolated virtual display; choosers are injected.
# Renderer/sandbox flags below are fixture-only, never production settings.
DSH_OCEAN_TEST_MAP_WEBVIEW=1 DSH_OCEAN_TEST_DIALOGS=1 GDK_BACKEND=x11 \
  WEBKIT_DISABLE_DMABUF_RENDERER=1 WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1 \
  timeout --kill-after=5s 90s bash scripts/test-native.sh xvfb-run -a \
  cargo test --test webkit_downloads --test webkit_multi_downloads \
  -- --ignored --test-threads=1 --nocapture
```

CI runs this test with a realized fixture window on Xvfb's virtual display and
the [private-XDG, non-activating D-Bus harness](../scripts/test-native.sh).
The local default remains hidden. Accepted native
Download objects are retained explicitly until their terminal signal, including
JavaScript/blob requests without a native caller. Only that
controlled fixture step disables WebKit's sandbox to accommodate hosted-runner
namespace restrictions; the shipped client never sets that environment flag.
The test driver bounds GLib dispatch as well as wall-clock time, so continuous
WebKit idle sources cannot hide a timeout.
