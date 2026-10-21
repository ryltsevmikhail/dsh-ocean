# Signed shell release verification, download and local activation

## Signed artifact download (explicit, no install)

```bash
dsh-ocean --download-release manifest.json signature.json trusted-key.json \
  --channel stable --target linux-x86_64 --kind appimage --min-sequence 42 \
  --output downloaded.AppImage
```

The public key must be authenticated/provisioned independently. The manifest and
signature are local bounded inputs; this command does not fetch a key or discover
a feed. It verifies Ed25519, release/channel/target/time policy and the remembered
sequence floor **before** starting any network request. The artifact URL comes
only from the authenticated manifest's canonical GitHub release location; no URL
argument is accepted. Download does not remember/advance the sequence floor.

System `curl` is required and trusted, as in the Linux bootstrap toolchain. Its
configuration file is disabled, TLS verification stays enabled, initial and
redirect protocols are HTTPS-only, redirects are capped at five, connect timeout
is 15 seconds and total transfer time is 180 seconds. Thirty seconds below
1 KiB/s aborts a stalled transfer. `--max-filesize` uses the signed size; the Rust
stream independently enforces the exact byte count and SHA-256. HTTPS redirects
are delegated to curl, not restricted to a destination-host/IP allowlist. Normal
proxy/CA environment configuration still applies; PATH wrappers are trusted tools,
not an adversarial sandbox.

Bytes flow directly into the existing private staging engine. A matching body is
published only after curl exits successfully, its size/hash match, and staging
is synced. HTTP/curl failure, truncation, excess bytes, hash mismatch or an
existing destination refuses publication. Early failure kills/reaps curl; private
staging cleanup is best effort. Curl stderr is discarded, never echoed with URLs,
proxy credentials or paths. The published file is mode 0600, not executable.

The JSON receipt says `downloaded:true`, `artifactChecked:true`, `applied:false`
and gives `stagedPath`. Explicit AppImage activation and rollback below are the
next steps; download itself never executes bytes, changes the active selector,
restarts a service or accesses a profile. Tests drive a synthetic-key download →
apply → second release → rollback under private HOME/XDG/prefix, using a fake
trusted curl (no live HTTP or release publication). A real published signed feed,
production trust provisioning, GUI orchestration and signed core/bootstrap
updates are still separate M2 work.

## Local activation and rollback (explicit, no download)

`--apply-release MANIFEST SIGNATURE TRUSTED_KEY <policy> --artifact STAGED
--prefix DIR --confirm-apply yes` activates an **already verified local** artifact
into a private, user-owned prefix. Only `--kind appimage` is supported for
activation: `tar.gz` may be verified/downloaded/staged but is refused by apply
before creating a prefix. No archive extraction is implemented:


```
DIR/versions/<release>-<sequence>-<digest8>/dsh-ocean
DIR/current  -> versions/<…>      (atomic symlink swap)
DIR/.previous -> versions/<…>     (retained for rollback)
```

Checks before anything is written: the manifest signature/policy, the staged
file's size and SHA-256 against the authenticated manifest, the remembered
sequence floor, and that the prefix is absolute, owned by the current user and not
writable by others. The bytes are copied through a no-follow descriptor into a
fresh private file, `fsync`ed, marked 0755 and then **re-verified after landing**;
a mismatch or an existing non-matching version refuses the operation. The previous
activation is written to `.previous` before `current` is swapped, so an explicit
`--rollback-shell --prefix DIR --confirm-rollback yes` can switch back. The
remembered release sequence is recorded only **after** a successful activation.

Limits: no download, no service restart and no process replacement — a running
client keeps the executable it already started. Rollback only switches the
selector back to one retained version; it is not a filesystem transaction, does
not remove newer versions, and does not verify that the retained version is still
healthy. Nothing here signs, attests or publishes; a system-wide install, package
manager integration and update-feed configuration remain unimplemented.

## Persisted anti-rollback state (opt-in acceptance)

`--verify-release`/`--stage-release` remain stateless and read-only apart from an
explicit staging output. `--accept-release` runs the same signature/policy checks
and then **remembers** the accepted `(channel, sequence, release, manifest
digest)` in a private state file:

```
$XDG_STATE_HOME/dsh-ocean/release-state.json   # mode 0600, schema 1
```

Rules enforced against the remembered entry for the same channel:

* an older sequence is refused (`refusing release rollback`);
* the same sequence with a different manifest digest is refused (`different
  manifest for already accepted sequence N`);
* a higher sequence is still refused when the release version is older
  (`refusing release downgrade`), which blocks malicious signed downgrades;
* re-accepting the identical manifest is idempotent.

The store is written atomically (private scratch + `rename` + directory `fsync`)
under an exclusive `flock`, so a rejected or failed update leaves the previous
state untouched. A corrupted or oversized store is reported and **blocks**
acceptance instead of silently resetting the floor. `--release-state` prints the
remembered entries; `--reset-release-state --confirm-reset` forgets them and
therefore re-allows older signed manifests.

Limits: this is local replay/downgrade protection, not a signature, not a trusted
clock, and not a production trust root. A same-user attacker can delete or edit the
file, and nothing here verifies that a *newer* manifest was actually published
(no freshness/consensus). Deleting the file with `--reset-release-state` is an
explicit operator decision, not an automatic recovery path. AppImage local
activation is explicit; tar extraction and update-feed configuration remain
unimplemented.


## Implemented boundary

Ocean has a read-only `--verify-release` CLI and a strict Ed25519 verifier.
It authenticates an exact shell-release manifest, checks caller-selected policy,
and optionally hashes an already-present local artifact. That verifier performs
no download, install, service operation, profile access, key discovery, automatic
signing, remembered sequence or self-update. It works headlessly without Node/runtime/GUI
initialization. A separate explicit `--stage-release` command can create a
verified local copy; it does not change the read-only verifier contract.
**Existing core/bootstrap update paths are still unsigned.**

There is **no embedded production trust root** or configured signed feed.
The operator must authenticate/provision the public key independently. A public
key fetched alongside an untrusted manifest proves nothing about its publisher.
Never use the deterministic test keys as production keys; no production private
key, GitHub secret or release was created by this implementation.

## Format v1

Three separate bounded, ordinary single-linked files are supplied. Final-component
symlinks, nonregular files and oversized inputs are refused; parent-directory
symlinks are not treated as a sandbox boundary. Manifest limit is 64 KiB;
key/signature files are limited to 4 KiB each.

Trusted public-key file (placeholder, not a usable trust root):

```json
{"version":1,"keyId":"release-root-1","publicKeyHex":"<64 lowercase hex characters>"}
```

Detached signature file:

```json
{"version":1,"keyId":"release-root-1","signatureHex":"<128 lowercase hex characters>"}
```

The algorithm is fixed Ed25519 with strict verification using `ed25519-dalek`.
Weak public keys, noncanonical scalar encodings and unknown formats fail closed.
No algorithm negotiation, key URL, PEM/private-key import or compatibility mode
is accepted. The exact signed message is:

```text
UTF-8("DSH Ocean release manifest v1\n") || raw_manifest_file_bytes
```

The prefix contains one LF byte. JSON is **not canonicalized**: whitespace,
encoding and line endings are covered by the signature. Signature verification
happens before manifest parsing. Unknown/duplicate struct fields are rejected;
errors do not echo raw JSON, key material or untrusted URLs.

Example manifest shape (dummy digest; not a signed release):

```json
{
  "version": 1,
  "product": "dsh-ocean",
  "channel": "stable",
  "sequence": 42,
  "release": "0.2.0",
  "publishedAt": "2026-10-19T09:00:00Z",
  "expiresAt": "2026-10-20T09:00:00Z",
  "artifacts": [{
    "target": "linux-x86_64",
    "kind": "appimage",
    "url": "https://github.com/ryltsevmikhail/dsh-ocean/releases/download/v0.2.0/dsh-ocean-0.2.0-x86_64.AppImage",
    "size": 123456,
    "sha256": "0000000000000000000000000000000000000000000000000000000000000000"
  }]
}
```

## Policy and integrity

- Product/schema must match. Channels are `stable` / `next`; a stable release
  cannot have a prerelease suffix. These are shell-feed channels, not a renaming
  of existing npm core channels.
- A semantic version must be at least this verifier binary's shell version.
  Build metadata is rejected. The signed sequence must be positive and at least
  the explicit operator-provided minimum. Equal sequence is allowed for repeat
  verification; no persistent highest sequence or sequence-to-digest pin exists.
- Publication can be at most five minutes ahead of the supplied/current UTC
  clock. Expiry must be after now/publication and no more than 30 days after
  publication. This assumes a trustworthy clock, not protection from clock rollback.
- At most four unique `(target,kind)` artifacts: `linux-x86_64` /
  `linux-aarch64`, `appimage` / `tar.gz`. Recognizing a target is not a claim
  that its package or engine has passed the portability matrix.
- Every artifact must use the exact canonical GitHub repository/tag/basename:
  `https://github.com/ryltsevmikhail/dsh-ocean/releases/download/vVERSION/dsh-ocean-VERSION-ARCH.EXT`.
  Arbitrary hosts, queries, fragments, credentials, traversal/encoded aliases
  and filenames are not accepted. Verification is metadata validation only;
  the separate downloader delegates bounded HTTPS redirects to trusted curl.
  No private-repository authentication is implemented.
- Nonzero declared size is capped at 1 GiB, with a lowercase SHA-256 digest.
  Optional local artifact verification uses one no-follow/nonblocking descriptor,
  checks regular type/single link/exact size, hashes bytes, rechecks descriptor
  size/times/link count and pathname identity. It changes no mode or bytes.
  Integrity is a point-in-time result, **not an immutable snapshot or a safe
  later install**. Another writer or pathname replacement after checking can
  invalidate it. Future apply code must use private staging and verified bytes
  without a path reopen race, plus its own extraction/publication policy.

## CLI

Run from a directory containing independently authenticated inputs:

```bash
dsh-ocean --verify-release manifest.json signature.json trusted-key.json \
  --channel stable --target linux-x86_64 --kind appimage \
  --min-sequence 42 --artifact downloaded.AppImage
```

Input filenames above are examples, not installed defaults. `--artifact` is
optional; without it only manifest signature/policy are verified. Output states
`publisherTrust: operator-supplied-key-only`, `artifactChecked` and `applied:false`.
It must not be interpreted as "the official update is installed" or as durable
anti-rollback state. A failure is nonzero and does not alter the supplied files.

## Explicit local staging, not apply

```bash
dsh-ocean --stage-release manifest.json signature.json trusted-key.json \
  --channel stable --target linux-x86_64 --kind appimage --min-sequence 42 \
  --artifact downloaded.AppImage --output verified-copy.AppImage
```

This authenticates the manifest/policy before any staging writes. Both source
and output are required. It opens a bounded ordinary single-linked local source,
then streams bytes into a new mode-0600 file in a private same-filesystem mode-0700
temporary directory. The exact copied stream must match signed size/SHA-256;
it does **not** verify a path and then reopen it for copying. Truncation, extra
bytes, read/write errors and wrong hashes fail without publishing a destination.
The copy is synced and atomically published with no-overwrite semantics using
the existing held-directory-descriptor transfer backend. Existing/racing targets
win untouched; private staging cleanup is best effort. Original source bytes and
mode are never changed, and no execute permission is added.

The library returns a held **read-only descriptor** for a future consumer, so
later source-file edits do not alter the copy. This is not an immutable/sandboxed
file: a hostile same-UID process can tamper with owner-controlled storage, and
an output pathname can be replaced later. Future apply code must retain/use the
verified descriptor/private storage and enforce its own final integrity and
installation policy rather than reopening a reported path blindly.

The CLI only reports `stagedPath` and still returns `applied:false`. It does not
launch, extract or install the artifact, remember a sequence, discover trust,
download anything, change a profile or touch services. `--verify-release`
rejects `--output` and remains read-only. Example paths are operator-specified,
not defaults in the installed app.

## Unsigned release draft

`--draft-release` reads one to four local artifacts and prints an **unsigned** v1 manifest
on stdout, with an explicit warning on stderr. It has no private-key, signature,
trust-store, output-file, staging or apply option. The API returns `Manifest`,
never `Verified`; authentication remains impossible without a separate signature.

```bash
umask 077
set -C
dsh-ocean --draft-release --release 0.2.0 --channel stable --sequence 42 \
  --target linux-x86_64 --kind appimage --artifact local-build.AppImage \
  --valid-hours 24 > manifest.json
```

For multiple artifacts, use repeated `--asset TARGET KIND FILE` groups instead
of the single-artifact `--target` / `--kind` / `--artifact` flags:

```bash
dsh-ocean --draft-release --release 0.2.0 --channel stable --sequence 42 \
  --valid-hours 24 \
  --asset linux-x86_64 appimage local-x86_64.AppImage \
  --asset linux-x86_64 tar.gz local-x86_64.tar.gz \
  --asset linux-aarch64 appimage local-aarch64.AppImage \
  --asset linux-aarch64 tar.gz local-aarch64.tar.gz > manifest-multi.json
```

Do not mix the two input forms. Incomplete groups, duplicate target/kind pairs
and more than four groups are rejected before source reads. Each artifact has
its own size/hash and canonical URL. JSON is emitted only after every source
passes; a bad later file produces no partial manifest or filesystem output.
Source order is retained. The original single-artifact CLI and API remain
supported. No claim that ARM/AppImage/archive builds are portable follows from
being able to generate their metadata.

Paths are examples, not installed defaults. Channel/target/kind and a positive
sequence are explicit. Publication time is current UTC, expiry is 1–720 hours
later. Shared verifier policy rejects bad versions, stable prereleases, schema/
channel/time/sequence/size violations; this is **policy validation, not trust**.
The operator must manage sequence monotonicity and the clock independently.
Draft generation does not remember previous releases or create anti-rollback state.

The command streams each nonempty regular single-linked source (at most 1 GiB each)
through the same no-follow/nonblocking digest routine as artifact verification,
with descriptor metadata/path identity rechecks. It derives the canonical
repository/tag/architecture/kind filename URL, size and SHA-256 without including
the local source filename/path. Pretty JSON plus its output newline is bounded
to the manifest size limit. Source bytes/mode are unchanged, no directories/files
are created by the command, and no Node/runtime/GUI/service initialization occurs.
The resulting digest is a point-in-time observation, not a frozen artifact:
verify the exact final release artifact again before publication/apply. An
artifact kind is metadata, not archive/ELF inspection or extraction safety.

Generating a draft does not publish the referenced URLs, prove binaries are
safe/portable, install an update or sign
anything. Sign exact output bytes with the separate publisher tool, then run the
verifier against the independently authenticated public key and artifact.

## Explicit offline publisher tool

The [Node signer](../scripts/sign-release.mjs) signs exact manifest bytes with
the same domain prefix and emits a JSON bundle to stdout. It **does not create
keys**, write output files, publish releases, install a trust root or contact
network/profile/service APIs. Its public-key output is for independent comparison,
not self-authenticating publisher identity. It also emits a raw-public-key SHA-256
fingerprint and manifest digest.

The operator supplies an existing **unencrypted Ed25519 PKCS#8 PEM**, stored
outside the checkout with current-user ownership and exact mode `0600` or `0400`.
Single-link regular-file, no-follow/nonblocking, bounded (8 KiB) descriptor reads
and before/after metadata checks reject unsafe inputs. Manifest input is bounded
to 64 KiB. Encrypted/other-algorithm keys are refused; no passphrase-on-argv/env
mechanism, key generation, HSM or secret-manager integration is provided.
Native fs/OpenSSL errors, paths, private bytes and stacks are not echoed;
broken output pipes also fail without an unhandled Node stack. PEM
buffers are overwritten best effort after use, but JS/native GC and OS memory
copies are **not guaranteed erased**. This is not a key-custody sandbox against
a hostile same-UID process or filesystem.

Example using an independently managed operator key (no key is bundled here):

```bash
umask 077
set -C # refuse accidental shell-redirection overwrite
node scripts/sign-release.mjs --manifest manifest.json \
  --private-key /operator-managed/path/release.pem --key-id release-root-1 \
  > signing-bundle.json
jq '.signature' signing-bundle.json > signature.json
jq '.publicKey' signing-bundle.json > candidate-public-key.json
```

All paths above are examples. Keep actual private keys outside version control,
CI logs/artifacts and this repository. Do not take the candidate public key as
trusted just because it accompanied a signature: authenticate its full key or
fingerprint over an independent channel first. Always run the Rust verifier with
that independently trusted key, explicit policy and optional artifact before
publishing. The signer intentionally does not validate manifest policy and can
sign bytes the Rust verifier will subsequently reject. No production key was
created/read or used by implementation tests; tests use ephemeral or deterministic
**test-only** material.

```bash
node --test tests/release_signer.mjs
```

## Evidence and remaining work

[Verifier/CLI tests](../tests/release_feed.rs) use deterministic **test-only**
keys and synthetic files. They cover valid signatures, exact-byte/domain binding,
wrong key/identity, weak keys, malformed/oversized data, unknown/duplicate fields,
channel/version/sequence/time/URL/size/hash failures, linked files, unchanged
artifact modes/content and a headless CLI with no runtime on PATH. Node's native
crypto signs a fixture that Rust strictly verifies. Node is not required by the
Rust verifier/stager. The optional publisher tool uses Node built-ins only.
[Signer tests](../tests/release_signer.mjs) additionally cover exact-byte/domain
binding, public-only output, unchanged key bytes/mode, owner-read-only keys,
unsafe modes, symlink/hardlink/FIFO and oversized/corrupt/wrong-algorithm inputs,
argument validation and coarse CLI diagnostics. Rust verifies a bundle from the
actual tool against its independently known test public key. The integration
chain starts from the actual headless unsigned-draft CLI and then invokes the
actual signer and strict verifier with both original artifacts. Draft tests cover
all four unique target/kind pairs, independent size/hash verification per source,
canonical URLs, unchanged source data, no private local-path output, duplicate/
excessive/empty groups before source reads, no partial CLI output on a later-file
failure, legacy single-input compatibility, mixed/malformed CLI rejection,
bad policy, oversized metadata, empty/linked/nonregular artifacts and the fact
that a draft alone is not an authenticated proof.
Owner-mismatch and
concurrent mutation branches are inspected but not explicitly fault-injected.
Staging tests cover an independent read-only copy, source edits after copying,
unchanged source mode, private output mode, cleanup after interrupted/truncated/
extra/wrong bytes, a deterministic late destination race, retained-descriptor
integrity after pathname replacement, invalid-manifest no-publication, CLI
no-overwrite and rejection of staging flags by the read-only verifier.
Private I/O unit seams also simulate `ENOSPC` before/after partial writes,
`WriteZero`, another write error, and `EIO` from pre-publication sync. They prove
that write/hash failures never reach sync/publication, partial payloads are
cleaned, and even a late user target survives a sync failure. Production uses
only `File::write_all` / `File::sync_all`; no environment/remote/auto-apply
fault-injection hook exists. Tests neither fill the disk nor change system limits.
Focused independent read-only reviews found no blocking verifier/staging flaw
within the stated boundary; these are not cryptographic audits or installer
reviews. A pre-open RAII guard now captures the new transfer directory's identity
without needing another descriptor and removes the exact empty directory if
opening/initialization fails (including injected `EMFILE`). It validates the
opened descriptor and named directory before handing cleanup to `Transfer`.
Replacement paths and unexpected contents are preserved, never recursively
removed by the initialization guard. If even the initial identity lookup fails,
or cleanup is denied, a directory can still remain rather than risk an unverified
delete. I/O failures above are simulated at the private operation boundary;
real hardware/filesystem failure, device quotas, power loss and the kernel's
actual fsync durability behavior are not validated by those tests.

```bash
cargo test --test release_feed
cargo test --lib staging_tests
```

Production key custody/provisioning/rotation/revocation, signed publishing CI,
feed fetch/redirect policy, persisted rollback/freeze state, authenticated
network downloader, safe shell apply/rollback, package-manager behavior and GUI acceptance
are **not implemented by this slice**. M2 remains incomplete.

The declared Rust floor is now 1.90: the already-locked Tauri 2.12 stack itself
requires 1.90, whereas the previous 1.77 declaration was inaccurate. The selected
Ed25519 library requires only 1.85. CI currently builds with current stable Rust;
a separate full minimum-toolchain matrix has not been run.
