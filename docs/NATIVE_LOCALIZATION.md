# Built-in localization (development)

Release **0.1.1 remains optional/manual**. This development branch does not change
that release, its assets, signatures or immutable tag.

## Ocean

Eight embedded catalogs cover English, Russian, Spanish, Brazilian Portuguese,
German, French, Japanese and Korean. English is the per-key fallback. The native
language picker saves a separate, versioned `localization.json`; it does not
rewrite server configuration. Changes take effect after restarting Ocean.

The startup language is a process-local immutable snapshot. System selection uses
`LC_ALL`, then `LC_MESSAGES`, then `LANG`; Ocean does not change the C locale or
these environment variables. Owned setup/pairing pages get plain catalog data
through guarded native IPC. The external Harness window cannot open the native
language-setting control or request those catalogs.

Message arguments are substituted once. GTK labels and web text use plain-text
APIs. The pairing QR SVG is a separate native-generated trusted value, not a
translation sink. Notification bodies are escaped for freedesktop markup.
Read-only storage and plugin overviews now localize their titles, close controls,
counts, empty/error states and safety notices. Their numeric/path filtering and
non-destructive behavior are unchanged. Storage reports that names are not shown,
not that directory enumeration never encounters names; file contents remain unread.
Download history also localizes its six terminal/live state labels, progress,
destination/staging captions, selection/cancel controls and metadata-clear notices.
Control/bidi escaping and single-pass path insertion remain in place; cancellation
IDs, transfer limits, publication gates and file-retention behavior are unchanged.
Backend diagnostic details and ISO timestamps remain literal technical data.
Component tests cover all eight catalogs; private Xvfb/non-activating D-Bus tests
exercise the native download/storage/plugin controls in the default English locale.
Read-only snapshot verification now also localizes its picker, bounded progress
stages, cooperative-cancellation notices and integrity summary. Invalid timestamp
metadata is represented as unavailable data before localized display; it is never
repeated as raw text. Checksums remain explicitly distinguished from signatures,
publisher authentication, immutable snapshots and data-schema rollback. No restore,
service or package operation is added. Eight-catalog component tests and a private
native GTK acceptance test cover these presentation changes.
Backup history separately localizes configuration and whole-data metadata rows,
empty/error states and authenticity/offline-operation notices. Configuration-backup
creation localizes its update-busy warning, destination receipt and result headings;
existing update guards, private storage creation and redacted technical errors are
unchanged. No restore, prune or deletion action is added to the history window.
Other native panels and status summaries are still being localized.

## Harness resource tree

The source catalogs and registration logic remain in `language-pack/`. Generate
and check the embedded resource snapshot with:

```sh
node scripts/build-builtin-locales.mjs
node scripts/build-builtin-locales.mjs --check
node --test tests/builtin_locales.mjs
```

The checked-in snapshot in `locales/dsh/` avoids adding Node or network access to
Cargo builds. Rust includes the seven resource files directly; no optional tarball,
npm install or downloaded language supplement is needed for that data. The
SHA-256 resource identity uses sorted paths and fixed-width big-endian length
prefixes, so it is independent of native CPU architecture.

The host entry is a no-op. The manifest exposes the official web Client entry;
its lazy factory registers the seven languages and their dictionaries through
Cordis-owned effects, with upstream English fallback. It does not call
`setLocale`, change a host preference or install a dependency. The MIT copyright
and licence are retained both in the embedded tree and the native package's
explicit licence payload.

The low-level cache helper now publishes an exact, sealed, content-addressed
resource tree using atomic no-replace rename. Existing conflicting trees, links
and unexpected entries are refused rather than repaired. Its caller must first
establish owned-start, pending-operation and profile-consent gates; the cache is
not inside a signed runtime generation.

The separate versioned consent store binds an explicit allow/refuse choice to an
absolute web-profile path and the pinned SDK's generated-root contract. Absence,
a changed scope, invalid data or a future document never grants permission. The
store does not create the profile or grant process/unit ownership. The native
approval dialog now provides explicit allow/refuse actions, conservative default,
plain profile text, and operation-stamp checks before display and persistence.
Dismissal never approves; invalidation closes the outstanding dialog and late
responses are ignored. The dialog is not exposed as a webview command. The native
owned-start planner now calls this dialog and requires its matching persisted
allow record before materializing resources or running metadata inspection.

Two embedded preflight helpers now classify configured potential legacy language
packs using resolved manifest identity, including file-based entries and literal
group disable ancestry. They use public SDK detached-composition APIs, not Loader
boot or configured-plugin imports. A fresh profile is synthesized in memory;
existing user patches are retained as input. Missing scope, warnings, unresolved
modules, dynamic names or legacy/group conditions, and nested includes return
`unknown`, never absence. Unrelated stock-plugin disable expressions do not decide
whether a legacy locale package is configured.
These helpers now pass private complete-SDK tests for fresh web profiles,
configured legacy bundles, explicit disabled policy, dynamic legacy conditions
and nested includes, without profile creation or authoritative-file rewrites.
The metadata subprocess requires Node's documented
`--experimental-import-meta-resolve` flag to resolve against the explicit profile
base and releases its public Cordis root fiber afterwards.

An opt-in isolated production-child test also boots the embedded overlay twice,
verifies its package in the actual authenticated client boot graph, preserves the
stock empty generated root and avoids adding profile dependencies. It checks
restart authentication and descendant cleanup. It now obtains the overlay through
the recorded-permission/actual-scope planner rather than passing a cache path
directly. This is not yet a real Tauri-window or native user-manager integration
test; the application call sites and manager transitions are tested separately.

The Rust metadata subprocess wrapper now supplies an explicit executable, script,
profile scope, process home and cwd. It clears inherited credentials and Node
startup injections, bounds both output pipes to 8 KiB and the operation to 30
seconds, kills/reaps failures and rejects extra protocol fields. An opt-in test
executes the actual embedded helpers through this wrapper against the private SDK
without creating a profile. The planner binds it to the selected executable's
nearest SDK manifest, actual launch HOME/DSH_HOME/cwd and scoped native permission.
Relative/empty overrides, unsafe directory ancestry and profile aliases are
refused rather than silently selecting another profile. The matching grant is
re-read after metadata inspection: revocation while that subprocess runs cannot
produce a new activation recipe. This is a cooperative latest-state check, not a
claim to lock settings against hostile concurrent edits.

Published owned-server metadata now retains the actual overlay path atomically
with its child. Ordinary child restart and token refresh preserve that recipe.
A service restart or fresh token does not prove ownership: Attached metadata stays
Attached. Refresh requires captured metadata for the same origin and refuses a
missing server or child-to-service transition instead of inventing a plain owned
recipe. Already-owned systemd metadata retains its exact overlay path.
The live runtime-switch controller captures separate old, candidate and active
recipes; candidate planning is restricted to before ownership/stop. Its systemd
ownership comparison accepts only the explicitly captured patched template (or
its exact historical pre-UMask image), with unchanged manager/provenance guards.
Tests cover child and systemd candidate/rollback roundtrips, including a plain
candidate restoring an old patched recipe. Private pending journals now retain
old/candidate recipes and restore them before recovery ownership checks. Legacy
journals default to plain recipes; plain transactions omit the new fields so
legacy strict readers retain their original schema. Malformed relative,
parent-escaping or non-overlay paths are refused before transaction mutation.
Same-session child recovery and fresh-session systemd recovery retain the saved
old recipe rather than consulting current preferences or candidate metadata.
Both owned cold-start backends now call the native planner after the existing
pending/reopen/Attached early gates. Only an explicitly approved, compatible SDK
with metadata `absent` receives the overlay. Systemd uses the manager's HOME and
DSH_HOME, but passwd `%h` for cwd. Its slot must be explicitly inactive, use the
exact known generated file and have matching fragment/no drop-ins before the
permission dialog or cache work. The proof is repeated before replacement and
start; refusal does not trigger a different backend. Missing units are published
without overwrite; exact known native units can return to plain on opt-out.
Runtime replacement re-inspects the imported SDK before stop/commit for an already
native server; a previously plain server is never implicitly enabled by update.
Rollback continues using saved recipes, never a fresh preference/preflight.

The client uses the pinned SDK's public module manifest and entry-state observable
to yield synchronously before host-managed graph reconciliation activates a manual
language pack. Initial activation order does not choose dictionary ownership;
manual removal restores native dictionaries only after reconciliation settles.
Existing conflicting language IDs are left untouched; partial native registration
failures dispose only native resources. Tests use strict duplicate-rejecting doubles
and the actual SDK LocaleRuntime/ClientModuleSystem with an explicit Loader double.
Arbitrary out-of-band module loads absent from the host graph are not prospectively
coordinated; no unsupported loader interception or order-sensitive override is used.

Cold systemd migration also recognizes an exact generated unit referring to a
retained schema-one cache. A bounded, read-only lookup validates only the matching
candidate: private owned parent, sealed exact seven-file inventory, no links,
bounded sizes, native package identity and the original length-prefixed SHA-256
content identity matching its directory name. This is local cache integrity, not
a signature or authority to execute old code. Cold launch still selects only the
currently approved compiled overlay or plain arguments; retained bytes are never
repaired, overwritten or substituted for a journal's captured rollback recipe.
Unknown or corrupt older paths remain refused before unit mutation.

Remaining validation includes real Tauri-window and user-manager integration,
ARM64 native runs, cross-binary recovery/native packaging and the complete current
packaging/license report.

**Release/native integration validation is not complete.** The implemented
activation callers and component tests do not by themselves prove release ELF
resource retention, real native-window behavior or ARM64 packaging; those checks
must establish the remaining completion evidence.

## Activation boundary to preserve

- Never materialize, inject, restart or rewrite an attached foreign runtime.
- Check operation/pending-recovery gates before creating resources.
- Preserve immutable runtime generations; resource caches must be independently
  content-addressed and retained while old generations can be rolled back.
- Use the supported CLI overlay mechanism, not private imports or copied boot
  internals. An overlay may be written into the SDK-generated `cordis.yml` by
  normal Loader writeback. Do not describe that as lifetime-ephemeral or as
  permission to modify an arbitrary profile.
- Legacy/shared profile activation needs explicit native, profile-scoped consent
  covering that generated root. No silent dependency, authoritative user-patch,
  preference or credential migration is allowed.
- Detect existing manual language packs before activation: identical language IDs
  or duplicate Client module manifests can conflict. Do not remove or patch a
  user's existing add-on to resolve that conflict.

The remaining implementation must prove these properties in isolated pinned
runtime fixtures and genuine native tests on both target architectures. New
production bytes still require a fresh scoped native/licence report and separate
owner approval before signing or publication.
