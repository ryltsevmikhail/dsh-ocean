# Signed complete runtime metadata — M2 boundary

This checkpoint implements runtime **metadata authentication/check/explicit
acceptance, authenticated complete-generation download/import, isolated candidate
health, exact home snapshots, a durable switch/rollback engine and proven-owned
Systemd/Child adapters, native signed check/download/apply and explicit recovery**.
The installed-update UI now uses the independent signed runtime source, not npm
version tags or a detached unsigned installer. Accepted M1 first bootstrap/manual
maintenance trust remains separate; a version pin is not a release signature.
Fresh-session Systemd recovery is supported; lost child ownership refuses safely
with the pending record retained (see limits below). Whole-M2 final review and
production trust/artifact/laptop acceptance remain separate gates.
Do not describe an npm version pin as authentication of its dependency closure.

## Independent authority

Runtime signatures authenticate:

```
DSH Ocean runtime manifest v1\n || exact UTF-8 JSON bytes
```

The detached envelope/key document use the same strict Ed25519 encoding as
[shell verification](RELEASE_VERIFICATION.md), but the domain and product differ.
A shell signature cannot authorize runtime metadata or vice versa, even with the
same public key. Operator settings contain a **separate** runtime source/key and
stable/next endpoints. No key discovery or production key is invented.

The runtime replay/downgrade store is
`$XDG_STATE_HOME/dsh-ocean/runtime-release-state.json`, with its own lock. It
cannot consume or advance the shell store. Both reject unsafe ancestry, symlinks,
writable/unowned/linked state and corrupt/oversized records; writes are private,
synced atomic replacements. These are local records, not a trusted clock or a
sandbox against the same UID.

## Version-1 schema

Illustrative only, **not an actual release or usable production configuration**:

```json
{
  "version": 1,
  "product": "dsh-ocean-runtime",
  "channel": "stable",
  "sequence": 9,
  "release": "0.2.1",
  "publishedAt": "<RFC3339>",
  "expiresAt": "<RFC3339>",
  "components": {"node": "26.12.0", "dsh": "0.2.1", "pnpm": "10.34.7"},
  "stateScope": "web-profile-sessions-storages-v1",
  "artifacts": [{
    "target": "linux-x86_64",
    "kind": "tar.gz",
    "url": "https://github.com/ryltsevmikhail/dsh-ocean/releases/download/runtime-stable-v0.2.1-r9/dsh-ocean-runtime-stable-0.2.1-9-x86_64.tar.gz",
    "size": 123456,
    "sha256": "<64 lowercase hex: complete runtime archive>",
    "nodeArchiveSha256": "<64 lowercase hex: original Node source archive>"
  }]
}
```

At most one complete tar artifact per supported architecture (x86_64/aarch64),
not separate mutable npm inputs. The bundle must ultimately contain all resolved
Node/DSH/pnpm/native dependency bytes. Original Node source checksum is signed
provenance, not a substitute for verifying the **whole** delivered archive.
DSH component version equals bundle release. Component versions are exact
semver without build metadata. Stable rejects a prerelease DSH release. Policy
shares bounded size, trusted sequence floor, expiry (maximum 30-day lifetime),
future-clock tolerance and downgrade refusal with shell metadata, but canonical
runtime URLs include channel/release/sequence so products and channels do not
silently overwrite one another's assets.

`stateScope` is a publisher compatibility declaration, not filesystem confinement
or proof of consistent backups. The identifier is bounded/recognized at this
checkpoint. The separate transaction snapshot primitive below can retain the full
controlled home, but actual owned-writer stop/orchestration, restore-before-old-start
ordering and supported migration acceptance remain M2 implementation/owner gates.
The declaration alone never establishes data rollback or write confinement.

## Authenticated complete artifact download

`--download-runtime-release` uses the same authenticated inputs/policy as Verify,
with `--output ARCHIVE --confirm-download yes`. Runtime-domain verification and
its independent floor precede transport; only the canonical URL from the selected
Verified runtime artifact reaches the existing bounded HTTPS-only curl source.
Exact size/SHA and successful transport exit are required before no-overwrite
private publication. Shell signatures cannot authorize runtime bytes. A failed
transport, including exit failure after all expected bytes, publishes nothing.
No code runs, generation is selected or floor advances. This ends that transfer's
publication/cancellation boundary; later preparation/apply starts a fresh operation.

## Complete-generation import (no activation)

`--import-runtime-release` uses the same authenticated inputs/policy as Verify,
plus `--artifact ARCHIVE --prefix DIR --confirm-import yes`. It captures exact
signed compressed bytes privately, rejects bad size/hash before extraction,
then extracts the full expected root with the existing bounded no-link archive
validator. It preserves all package/native files and notices, requires regular
executable Node/DSH/pnpm entries, and checks the four-line component/source digest
record. The publisher cannot supply transaction predecessor/receipt/index files.

A complete owned index is hash-bound to the private receipt, verified before
atomic no-overwrite publication under `generations/gen.<alphanumeric>`, and fully
rechecked before any retained generation reuse. Tampered retained content fails
without overwriting it. The prepared handle holds the existing `.bootstrap.lock`
through future health/activation so cooperating installers cannot race it. Local
failed staging cleans only its own private temporary directory. Install prefixes
must be separate from Harness state, not paths inferred from an install hint.

Import **does not execute Node/DSH/pnpm**, install packages, change `current`,
write `.previous`, restart a service, mutate a profile or advance the remembered
floor. It reports `healthChecked:false`; artifact authentication and a complete
retained tree are not proof of native version/ABI/authenticated health. A separate
explicit candidate check below supplies that evidence; owned controller/data
transactions remain the next M2 step. A CLI
import ends its lock when it returns; an inactive prepared generation may later
be reclaimed by an explicit/cooperating installer/GC.

## Isolated native/version/authenticated health

`--check-runtime-bundle` takes the same metadata/policy/artifact/prefix inputs,
with **`--confirm-check yes`**, imports/rechecks the complete retained tree and
executes only the authenticated **native Linux ELF64 Node**. It checks metadata
freshness again at execution/result boundaries, holds the installer lock, and
passes the exact signed component versions to the embedded helper. A changed
retained file or incompatible architecture refuses execution.

The helper probes exact Node/DSH/pnpm versions and loads `node-pty`/`koffi` from
the DSH package before server health. Every probe uses disposable HOME/DSH_HOME,
all XDG directories, TMPDIR/cwd and a pinned clean PATH; the Rust launcher clears
inherited environment/NODE_OPTIONS. The server binds 127.0.0.1 port 0, disables
browser launch and must return anonymous 401 then launch-token 303. No real
profile/server or global install is used. Token-bearing output is not returned.

The owned Rust process group bounds combined output (64 KiB) and total time
(95 seconds), polls cancellation without blocking GTK, and kills/reaps its owned
checker on every result/error/timeout. In this mode helper children inherit that
group; legacy standalone calls retain their existing group cleanup. This is
cooperating process ownership, not containment of malicious same-UID code or
processes that deliberately escape groups. Temporary state is removed only after
owned cleanup. A health result is bound to the staged manifest/components/tree;
**it is not an activation permit or data-migration compatibility guarantee**.

[Helper tests](../tests/runtime_health.mjs) exercise isolated probes, version/native
mismatch, authentication and cleanup. An opt-in [signed-bundle fixture](../tests/runtime_feed_cli.rs)
uses an explicitly selected real Node binary with synthetic DSH/pnpm/native module
stubs, fixed test-only trust and private state. This proves the complete client
orchestration, not real DSH/native-addon/production acceptance:

```bash
DSH_OCEAN_TEST_RUNTIME_NODE=/usr/bin/node cargo test --locked \
  --test runtime_feed_cli signed_runtime_bundle_health -- --ignored
```

## Transaction-only managed-home snapshots (controller not wired yet)

Unlike manual profile exports, the new internal transaction primitive captures
**all supported files within the controlled DSH_HOME**, including plugin modules,
configuration, sessions/storage and local credential files. This is conservative
rollback coverage beyond the declared minimal writable-state scope, not a claim
that arbitrary migrations/outside paths are supported. It does not read external
credential locations or follow links outside that home. Production update consent
must explicitly explain that a sensitive private local backup is being retained.
Snapshots are **not encrypted or exported**; directories are owned mode 0700 and
metadata is 0600, including on reopen. They must stay outside the managed home.

The stopped-writer callback is mandatory before capture/restore and at bounded
copy/publication checkpoints. Hash/size/mode/mtime/internal-link inventories are
checked after copying and before publication. Originally absent home is recorded
without creating it; an initially absent storage/config subtree is naturally absent
after full-tree restore. Reappearing roots/changed data refuse publication rather
than calling a concurrent snapshot consistent. This is not global exclusion of
unknown same-UID writers: the future controller must actually stop its owned
writer, hold its lifecycle/prefix guards, and require other clients/CLI writers
closed.

The implementation refuses outside links, nested devices, hard-linked regular
files, special files/bits, other-writable source state, non-UTF8/oversized trees,
files above 512 MiB, total data above 2 GiB, over 200000 entries or excessive
path/depth. It never silently skips such state. Byte/mode/mtime/link restoration is
not restoration of inode identity, hard-link topology, ACLs or extended attributes.
Those unsupported forms require an operator-managed migration, not a partial
automatic rollback disguised as success.

### Existing profile permissions and private creation

Ocean-owned web children create new files with a child-local `umask 0077`; newly
generated systemd units use `UMask=0077`. The GUI's own process mask is not changed.
This prevents a permissive inherited user-manager mask such as `0002` from making
new session locks or other normal DSH state group-writable. DSH subprocesses inherit
that policy; independent CLI commands and attached/external servers do not.

**Existing files are not repaired or chmodded.** A profile already containing
`0664`/`0666` files or group/other-writable directories remains ineligible for an
automatic transaction snapshot. Before retrying, stop every DSH writer, verify the
actual profile/home and ownership, retain a trusted backup, and review the affected
permissions individually. If shared write access is intentional, use an
operator-managed migration instead. Do not use a blanket recursive chmod, follow
outside links, or weaken the snapshot check to make an update pass. For separate
CLI provisioning of a new private profile, use a restrictive mask in that CLI's
own shell before it creates data; it is not a repair of existing permissions.

Previously generated units without `UMask` are recognized **only when every byte
matches the exact historical generated template**, with the same fragment/no-drop-in
and manager-home proofs. Their exact bytes are retained for rollback; a successful
owned runtime update installs the new private-policy after-image. Merely attaching
or upgrading the GUI does not rewrite a running unit or change old file modes.
Custom units, including a different `UMask`, remain outside automatic ownership.

Restore first verifies the private snapshot and builds a verified replacement in
a private sibling container. Linux atomic directory exchange switches a present
home; an absent original home is reestablished as absent. Candidate-created state
is moved into a **private retained container**, never silently deleted, including
after a post-exchange error. A failed offline guard before publication leaves live
state unchanged; corruption refuses replacement. The future transaction controller
must keep the runtime stopped and the recovery journal/backups on **any** incomplete
restore, and only restart the old runtime after restoration succeeds. Existing
manual backup/restore formats and their intentional omitted-root behavior do not
change.

[Private filesystem tests](../tests/runtime_snapshot.rs) cover full config/plugin/
credential/session restoration, internal links, missing home/new storage removal,
retention of candidate data, writer-guard failures, corrupt snapshots, other-writable
state and absence/publication consistency. All contents are synthetic; no running
user profile or service participates. No live update controller/automatic rollback
is claimed by these primitives alone.

## Durable switch engine, owned OS adapters and native orchestration

[The shared engine](../src/runtime_switch.rs) accepts typed Prepared/Verified and
runs the native candidate check before entering a noncancellable local commit.
The narrow trusted controller boundary must represent **proven owned** Systemd
or Child, not Attached/loopback guesses. The real adapters and configured native
entrypoints below share this transaction and its pending-recovery gate. Callers must retain the settings, Runtime floor guard,
Prepared prefix lock and lifecycle permit, and supply the exact atomic floor
record callback. A controller health result must mean authenticated live health.

The engine fsyncs a private pending journal before stopping, proves the controller
stopped, captures the real full-home snapshot and persists its pointer before
changing the relative generation selector/unit. It starts/checks the candidate
and records the monotonic floor only on success. Candidate failure stops it first,
restores data/absence before old selector/unit and **only then** starts/checks the
old runtime. Restore/stop/unknown selector or unit failures retain pending recovery
and backups; they never intentionally restart old against unrestored data.
The owned systemd adapter does not treat `systemctl start` / `Type=simple` as
application readiness. Candidate, rollback and explicit recovery share a bounded
30-second authenticated wait: the anonymous origin must return 401 and the current
launch-token exchange must return 303. Journal candidates are refreshed from the
fixed owned unit; journal output, helper lifetime and HTTP probes are bounded by
the readiness budget (failed helper cleanup may take up to two additional seconds).
This uses stock `journalctl --no-pager`, not wrapper scripts which detach their
own workers: timeout/oversize cleanup signals the retained helper group, while an
already exited helper is reaped without signalling a recycled numeric group ID.
Only successful authenticated readiness publishes the new server/token. A permanent
failure still follows the existing rollback/pending-recovery rules; waiting is not
permission to accept an anonymous server or to clear an incomplete transaction.

Explicit recovery uses the private home/backup-bound journal and refuses foreign
selector/unit images or a new selector without a durable snapshot. Current engine
support requires an existing `generations/gen.<alphanumeric>` selector; legacy
layout migration and explicit post-success history/rollback are separate work.

Ordinary local prepare/restart/health-monitor recovery now refuse pending entries,
even corrupt/dangling ones; remote attachment is not made an update authority.
Only explicit transaction recovery may proceed with a pending record. The bundled
setup window shows **Recover runtime…** instead of Retry/Install when the local
pending gate is present. It re-inspects the journal and opens the same native
confirmation as tray → Check for DSH core updates; declining changes nothing.
Malformed journals still produce an inspection error, not an automatic repair.
If another operation already cleared the journal, the action reports that nothing
is pending: it never falls through to a network update check. No new
remote-webview updater API/capability is exposed.

[Filesystem transaction tests](../tests/runtime_switch.rs) use real snapshots,
selectors and synthetic data with a fake owned OS controller, not mocked backups.
They exercise failure ordering, restore refusal, original absence, interrupted
recovery, ownership interference, floor callback errors, cancellation commitment,
changed recovery scope and local-start gates. Those engine tests do not prove real
OS adapter behavior; that separate boundary is tested below. Genuine publisher
runtime acceptance remains an owner gate. The installed core update UI now calls
the signed native facade described below; no npm fallback is offered.

### Proven-owned Systemd and managed-child controllers

[OwnedController](../src/runtime_controller.rs) retains the lifecycle permit,
captures the explicit old runtime/root and fixes its mode before mutation.
`owned(app, permit, old, candidate_root, expected_home)` runs on a blocking worker;
`headless` uses the same lifecycle without navigating a window. Attached and
external/legacy runtimes are refused; a local address alone is not ownership.

Child mode leases only Ocean's actual child handle. Successful stop reaps it
before discarding; failed stop restores the lease and refuses replacement. Start
uses the explicitly requested root, never rediscovery through the new selector.
Systemd requires the exact generated old unit bytes, exact manager FragmentPath
and no DropIns. Checked replacements accept only known old/new images and reload
and recheck the manager; unknown concurrent edits refuse rollback too. These are
cooperative ownership/CAS checks, not hostile same-UID race exclusion.

Before an owned Systemd transaction, the expected snapshot home must match the
**effective user-manager** HOME/DSH_HOME scope, not merely the client environment.
A bounded (64 KiB, five-second) manager query filters and retains only those two
values, discards stderr and never reflects values in errors. Missing HOME falls
back to the account home; absent DSH_HOME uses HOME/.dsh. Mismatched, duplicate,
quoted/escaped, relative/empty or unavailable scope fails closed. Scope is checked
again at ownership and start. Only exact ActiveState `inactive` proves stopped;
failed/transitional/unknown states and query errors are not mistaken for stopped.

After new start and old rollback, health requires anonymous 401 **and** a valid
fresh token redirect (303). Server/token state is refreshed/published/cached, and
`finish()` reloads the native app's window; headless `finish()` does no navigation.
[Adapter tests](../tests/runtime_controller.rs) use private fake-manager fixtures
and real owned private children, covering failed stops, lease restoration, explicit
old roots, fresh tokens, manager provenance, scope divergence, inactive proof and
bounded query failures. They do not operate the user's actual service/profile or
constitute genuine publisher/system integration acceptance.

## Native installed-runtime workflow

The tray's installed-core update action calls [the native facade](../src/managed_runtime.rs)
through [native GTK dialogs](../src/update.rs), with no webview updater command.
It checks the independent configured runtime feed/key/channel and reports their
identity. Missing configuration is an error/guidance state, never an npm fallback.
An exact selected-generation import receipt can report **Current**; the receipt
is cosmetic status, not apply authority. Equal DSH version alone is insufficient:
a signed candidate can change Node/pnpm/native closure and remains **Available**.

External, legacy, Attached and Remote installs get check-only/original-manager
or migration guidance, not an Apply offer. Eligibility is re-proven before any
artifact download/import. Metadata check and apply use the native operation
registry (duplicate gate and quit cancellation/defer). The lifecycle permit is
retained through the apply workflow; filesystem locks order settings → runtime
floor transaction → prefix `.bootstrap.lock`.

Explicit confirmation covers restart, closing all other CLI/TUI writers, and a
full **unencrypted private** supported-home backup containing credentials, config,
plugins/dependencies and sessions. Native preparation downloads authenticated
bytes to an unselected private cache without prematurely latching commit, imports
under the prefix lock and runs isolated native health before stopping the old
runtime. A fresh policy/expiry check runs again **before new-root start**, even
if snapshotting took a long time. Rollback of the explicit old root remains
possible after candidate expiry. Only successful new authenticated health records
the exact verified runtime floor; rollback never lowers it. The window/origin/
token are refreshed after apply and successful rollback.

Pending recovery is offered before a normal update check, without requiring a
working feed/key. Recovery binds the private journal to the configured paths and
captured **old** root; it does not rediscover the selected candidate as the old
runtime. Fresh-session Systemd recovery proves the journal's before/after unit
images, manager fragment/no-drop-ins and service data-home scope even when startup
could not publish a server because the journal is pending. Its origin must be the
configured fixed loopback origin; no token is published until the restored old
root starts and passes authenticated health. A changed origin/scope/foreign unit
fails closed and retains pending state.

**Limits:** a fresh session that lost its managed-child handle cannot infer
ownership from a PID or executable path and refuses recovery with guidance. Stop
unknown writers and obtain offline recovery assistance; do not delete pending or
start an old binary against unrestored data. Automatic same-session failure
rollback is supported; post-success core `.previous`/history rollback is not this
transaction's feature. The M1 maintenance command is selector-only, not a data
rollback substitute. Mutating legacy maintenance/bootstrap modes refuse pending
signed transactions under the prefix lock; read-only check remains available.
[Facade tests](../tests/managed_runtime.rs) cover a real signed child apply→failed
health→data restoration and fresh-session fake-manager Systemd recovery, all in
private synthetic homes. Actual GTK/window integration and genuine publisher/
service/laptop acceptance are separate owner evidence, not implied by those tests.

## Publisher archive assembly (not release clearance)

[The publisher-only packager](../scripts/package-runtime.py) assembles an already
resolved, frozen per-architecture Node/DSH/pnpm tree. It never runs npm, resolves
client dependencies, downloads, signs, changes trust or starts/stops a service.
The supplied `.versions` must be the exact four-line component/source record with
its final newline and DSH version matching `--release`.

```bash
python3 -B scripts/package-runtime.py --root PREPARED_RUNTIME_ROOT \
  --output-dir NEW_ARCH_OUTPUT --channel stable --release VERSION \
  --sequence COUNTER --target linux-x86_64
```

The output is the canonical complete archive and `artifact.json` (target, URL,
size, SHA-256, original Node archive hash). Payload includes `runtime`, `harness`,
`tools`, `.versions`, common root aggregate notices and all files inside these
payload trees, including native binaries and dependency notices. GNU long-name
records support bounded dependency paths. Files/directories have deterministic
archive timestamps and no owner identities.

Normal JS bin symlinks are converted into regular executable shims using pinned
Node's `process.execve`: the real entry keeps its package-relative imports and
main argv, and replaces the same PID rather than spawning an orphanable server.
Other symlinks (including directory/outside links), special files, special mode
bits and known credential/transaction metadata inside the payload are refused.
Resolve unsupported supplier layouts explicitly in the controlled publisher build;
never fall back to a client-side npm rebuild. Existing output is not overwritten.

This is byte assembly, **not** proof that the supplier tree's `.versions` is true,
that native ABI/dependency closure is compatible, or that licensing is cleared.
A publisher must independently validate source provenance, actual native versions/
bindings, the signed manifest and the resulting archive with the Rust verifier/
importer and disposable native health before publication. Current tests exercise
synthetic native bytes, real private JS shim behavior and independent signed Rust
import (including long paths/notices); they are not genuine DSH bundle acceptance.
The [manual guarded release workflow](../.github/workflows/release.yml) now wires
pinned native producer builds and independent checks on both architectures, with
no production keys or binary uploads in default dry-run. Its protected operation
creates only a draft; actual execution/provisioning/clearance and final public
publication remain owner actions (see [release operations](RELEASING.md)).

## Offline publisher signing

The existing [offline signer](../scripts/sign-release.mjs) accepts explicit
`--product runtime` to select **`DSH Ocean runtime manifest v1\n`**. Omitting the
option retains the shell domain for compatibility; `--product shell` is explicit.
Unknown/duplicate product options refuse signing. Runtime and shell signatures
are not interchangeable, even with the same key/raw bytes. The signer emits only
signature/public material and raw-manifest digest, never installs trust or
publishes anything; manifest policy still requires the independent Rust verifier.

```bash
# Owner-operated publisher environment only; no production keys are in this repo.
node scripts/sign-release.mjs --manifest MANIFEST \
  --private-key OWNER_PEM --key-id OWNER_KEY_ID --product runtime
```

The private-key file must already exist with owned 0600/0400 permissions. Do not
put it in artifacts, source control, logs or a request to an assistant. Production
key/fingerprint provisioning and rotation remain independent owner inputs. An
opt-in test cross-checks signer output with the Rust verifier using ephemeral
**test-only** keys and private synthetic metadata:

```bash
DSH_OCEAN_TEST_VERIFIER="$PWD/target/debug/dsh-ocean" \
  node --test tests/release_signer.mjs
```

## Headless commands

```bash
dsh-ocean --verify-runtime-release MANIFEST SIGNATURE PUBLIC_KEY \
  --channel stable --target linux-x86_64 --min-sequence 1 \
  --current-runtime-version 0.2.0

dsh-ocean --accept-runtime-release MANIFEST SIGNATURE PUBLIC_KEY \
  --channel stable --target linux-x86_64 --min-sequence 1 \
  --current-runtime-version 0.2.0 --confirm-accept yes

dsh-ocean --check-runtime-update --current-runtime-version 0.2.0
```

Verify reads local files; configured Check fetches only bounded metadata using
the independently provisioned runtime key/channel. Neither checks npm, executes
payloads, writes the floor, accesses a profile or starts/restarts a service.
Explicit Accept only records authenticated metadata; it does not install it.
The current version in these advanced read-only commands is an explicit caller
input, not a claimed ownership discovery. Owned runtime orchestration must
recompute/revalidate actual state before mutation.

A configured candidate retains exact signed bytes, fingerprint, settings digest
and original current-version policy. Revalidation rejects changed configuration
and checks fresh time, native target and runtime floor. Caller must hold the
settings guard through final use, as for the shell path.

[Isolated CLI fixtures](../tests/runtime_feed_cli.rs) cover valid metadata,
wrong key/tampering, both cross-product signature directions, unsupported scope,
component mismatch, unsafe URL/hash/format/architecture, expiry, independent
floor/replay and separate configured channel/key. No real publisher artifacts,
user profiles, runtime execution or live HTTP are used.
