# Signed complete runtime metadata — M2 boundary

This checkpoint implements runtime **metadata authentication/check/explicit
acceptance**, not runtime installation or data rollback. Existing bootstrap/core
installer paths remain separate and unsigned until their M2 replacement is wired.
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
checkpoint; exact stopped-writer snapshot/absence inventory/restore enforcement
and supported migration acceptance are still M2 implementation/owner gates. No
whole-home/global-credential rollback or excluded-plugin rollback is implied.

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
