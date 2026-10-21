# Signed shell release verification, download and local activation

## Configured feeds, trust and channels

`--update-settings` reports read-only status, channel selections and public-key
fingerprints, never key bytes. Missing configuration means **unconfigured**, not
an automatic trust/download fallback. Settings are separate from Harness and npm:
`$XDG_CONFIG_HOME/dsh-ocean/updates.json` (owned atomic mode-0600 writes).

An operator imports a reviewed settings document and confirms every configured
product's public-key fingerprint from an independent authenticated channel. The
fingerprint is SHA-256 of the raw 32-byte Ed25519 public key (64 lowercase hex
characters), not a hash of PEM/DER formatting:

```bash
dsh-ocean --configure-updates reviewed-updates.json \
  --confirm-shell-key INDEPENDENT_RAW_ED25519_PUBLIC_KEY_SHA256
# Add --confirm-runtime-key FINGERPRINT when configuring a runtime source too.
# Replacing an existing key additionally requires --confirm-replace-trust yes.
dsh-ocean --set-update-channel shell stable
dsh-ocean --set-update-channel runtime next
dsh-ocean --check-shell-update --kind appimage
```

This example is schema documentation, **not published production configuration**;
replace the public-key placeholder with independently authenticated public data.
The illustrated feed locations have not been provisioned by these tests:

```json
{
  "schema": 1,
  "shellChannel": "stable",
  "runtimeChannel": "stable",
  "shell": {
    "key": {"version": 1, "keyId": "operator-provisioned-id", "publicKeyHex": "INDEPENDENT_PUBLIC_KEY_LOWERCASE_HEX"},
    "stable": {
      "manifestUrl": "https://github.com/ryltsevmikhail/dsh-ocean/releases/download/channel-stable/manifest.json",
      "signatureUrl": "https://github.com/ryltsevmikhail/dsh-ocean/releases/download/channel-stable/manifest.sig.json",
      "minimumSequence": 1
    },
    "next": {
      "manifestUrl": "https://github.com/ryltsevmikhail/dsh-ocean/releases/download/channel-next/manifest.json",
      "signatureUrl": "https://github.com/ryltsevmikhail/dsh-ocean/releases/download/channel-next/manifest.sig.json",
      "minimumSequence": 1
    }
  },
  "runtime": null
}
```

Runtime configuration has a separate source/confirmation; its verifier must use
a separate signed product domain/floor. Runtime workflow wiring remains M2 work.
Selecting its channel never changes npm `latest`/`next` settings. A null source
disables that updater without resetting any accepted floor. Private keys do not
belong in this document or application. Confirming a fingerprint copied from the
same untrusted feed is **not** independent publisher authentication.

Feed URLs must be canonical public HTTPS assets in this repository, with no
credentials, query, fragment, port override, encoded path aliases or other hosts.
Current settings/import files must be bounded, regular, single-link, owned and
not writable by others; directory symlinks/untrusted writable ancestors fail
closed. Existing unsafe/invalid settings are never silently treated as defaults.

A configured check uses bounded curl transport (64 KiB manifest, 4 KiB signature),
authenticates the exact bytes before parsing policy, checks native architecture,
channel/expiry/version and remembered floor, then reports metadata only. It does
not fetch a key/artifact, advance state, install or access the Harness profile.
Candidates retain exact signed bytes and a settings digest: changes to trust or
channels invalidate them, and expiry/floor must be rechecked at use. Configured
activation must hold the settings operation guard across final revalidation and
the state/prefix transaction so trust replacement cannot race that commit. Manual
`--verify-release`/`--apply-release` with an explicitly supplied key remain advanced
operator-selected trust, not an implicit use of configured publisher authority.

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

Native-operation backend APIs share cancellation/progress state for feed checks
and artifact downloads. Cancellation is linearized against final result/publication:
if it wins, named staging output is not published; once publication commits, a late
Cancel is refused rather than reporting verified published bytes as cancelled.
Cancellation signals only the exact owned curl process; its worker reaps it.
Transport-exit polling never holds the child mutex across a blocking wait, and a
native cancel request never waits behind worker cleanup. Hash/copy checkpoints
clean private partial staging on cooperative cancellation. Abrupt process death
can still skip cleanup; trusted wrappers/descendants are not a hostile sandbox.
The native GTK controller uses these backend APIs; see [managed destination and lifecycle policy](SHELL_UPDATES.md).

The JSON receipt says `downloaded:true`, `artifactChecked:true`, `applied:false`
and gives `stagedPath`. Explicit AppImage/tar activation and rollback below are the
next steps; download itself never executes bytes, changes the active selector,
restarts a service or accesses a profile. Tests drive a synthetic-key download →
apply → second release → rollback under private HOME/XDG/prefix, using a fake
trusted curl (no live HTTP or release publication). A real published signed feed,
production trust provisioning and signed core/bootstrap updates remain separate
M2 work. Native shell orchestration now provides explicit configured check,
download, managed apply/rollback/recovery and channel controls.

## Local activation and rollback (explicit, no download)

`--apply-release MANIFEST SIGNATURE TRUSTED_KEY <policy> --artifact STAGED
--prefix DIR --confirm-apply yes` activates an **already verified local** artifact
into a private, user-owned prefix. Both `--kind appimage` and `--kind tar.gz`
are supported; tar bytes are never mistaken for an executable. A tar package must
have the builder's `dsh-ocean-<release>-linux-<arch>` root and an executable
`dsh-ocean` entry. The updater retains the **whole package**, including notices;
it does not invoke its installer scripts.


```
DIR/versions/<release>-<sequence>-<digest8>/dsh-ocean
DIR/current  -> versions/<…>      (atomic symlink swap)
DIR/.previous -> versions/<…>     (retained for rollback)
```

Before executable/selector publication the signature/policy, exact staged bytes,
remembered floor and owned non-writable-by-others directory chain are checked.
A global release-state lock spans the final floor check, prefix activation and
state recording; a per-prefix lock serializes apply, rollback and recovery.
Bytes use the existing bounded/hash-checked no-overwrite staging engine, then
0755 permissions and a retained receipt binding the manifest digest, version,
sequence, byte size and SHA-256. Current/previous retained receipts and bytes
must validate before changing selectors; symlink directory aliases are refused.

Versions are prepared in an owned private temporary directory and published with
`RENAME_NOREPLACE` only after complete validation. Invalid archives cannot leave a
partially prepared active version or overwrite an existing version. Cleanup of the
exact owned temporary inode is best effort; abrupt termination/filesystem failure
can leave private inactive scratch material for investigation.

Tar/gzip handling uses standard crates with a raw-header preflight before logical
PAX/GNU path processing: extension bodies are capped at 64 KiB, paths at 4096 bytes
and 128 components, entry count at 200,000, individual files at 512 MiB and file data
at 2 GiB (with separately bounded stream overhead/padding). Links, sparse/special
files, traversal/non-canonical paths, duplicate entries, unsafe permissions,
corrupt/trailing payloads and missing entrypoints are refused. New content is
private and never merged into an existing directory. A retained installed-file
index is size/hash-bound by the local receipt (64 MiB maximum) and checks the exact
file/directory set, normalized modes and every file hash on reuse/rollback;
damaged notices or unexpected code files block selection just like a damaged
executable. These are local integrity checks, not fresh publisher signatures.

A synced `.activation-pending.json` records original and intended selectors before
the switch. If normal activation/state recording fails, the original verified
selectors are restored. If the process is killed or power is lost, later mutation
fails closed until the operator confirms:

```bash
dsh-ocean --recover-shell --prefix DIR --confirm-recovery yes
```

Recovery restores only journaled original selectors after checking retained bytes;
it does not reset the accepted sequence floor or guess whether to finish an
interrupted update. Malformed journals, changed selectors or damaged recovery
versions remain untouched and require investigation. A floor already committed
before a later durability failure is never reduced; reapplying the identical
verified release is allowed. A verified but inactive version can remain on disk.

`--rollback-shell --prefix DIR --confirm-rollback yes` verifies both retained
versions and swaps the selectors under the same journal/lock. It never lowers the
floor. The local receipt is protected app-owned state created from an authenticated
release, not a new signature or protection against a hostile same-UID process.
Legacy prefixes without receipts are not silently treated as verified installs.

Limits: no service restart or process replacement — the running client keeps its
original executable. Integrity verification is not an executable health check;
rollback does not restore profile data or remove newer versions. Journaling is
recoverable coordination, not a kernel transaction across directories, immutable
storage or a guarantee against hardware/filesystem corruption. Nothing signs or
publishes a release here; package-manager integration remains separate.

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
under an exclusive `flock`. Rejection and pre-rename failures leave previous state
untouched; a durability error after rename can leave the new floor recorded, so
activation recovery never silently reduces it. A corrupted or oversized store is reported and **blocks**
acceptance instead of silently resetting the floor. `--release-state` prints the
remembered entries; `--reset-release-state --confirm-reset` forgets them and
therefore re-allows older signed manifests.

Limits: this is local replay/downgrade protection, not a signature, not a trusted
clock, and not a production trust root. A same-user attacker can delete or edit the
file, and nothing here verifies that a *newer* manifest was actually published
(no freshness/consensus). Deleting the file with `--reset-release-state` is an
explicit operator decision, not an automatic recovery path. AppImage/tar local
activation, configured metadata checks and native shell GUI are explicit;
signed runtime transactions and real production publishing remain separate work.


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

Implemented CLI foundations now include persisted sequence/downgrade protection,
operator-provisioned trust and channel settings, bounded authenticated shell-feed
checks, artifact download and journaled AppImage/tar package activation/rollback.
The native GUI adds explicit managed enrollment, check/download/confirmed local
transactions, channel settings, cancellation and original-installer guidance.
These are not a complete M2 or an unattended updater. Production key custody/
provisioning/rotation/revocation, signed publishing CI, signed runtime orchestration,
real package/AppImage and published-feed acceptance remain open. Core/bootstrap
updates are still unsigned; local apply/rollback retain the recovery/durability/health limits
described above. A [separate runtime metadata verifier and floor](RUNTIME_RELEASES.md)
now exist, but do not yet convert those installers into signed transactions.
M2 remains incomplete.

The declared Rust floor is now 1.90: the already-locked Tauri 2.12 stack itself
requires 1.90, whereas the previous 1.77 declaration was inaccurate. The selected
Ed25519 library requires only 1.85. CI currently builds with current stable Rust;
a separate full minimum-toolchain matrix has not been run.
