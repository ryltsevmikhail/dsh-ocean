# Signed complete runtime metadata — M2 boundary

This checkpoint implements runtime **metadata authentication/check/explicit
acceptance, authenticated complete-generation import, isolated candidate health
and transaction-only exact home snapshot/restore primitives**, not yet a wired
live runtime switch/restart/data-rollback controller. Existing bootstrap/core installer paths remain
separate and unsigned until their M2 replacement is wired.
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
