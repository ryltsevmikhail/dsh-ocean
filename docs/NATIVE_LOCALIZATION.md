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
Remaining native panels and status summaries are still being localized.

## Harness resource tree

The source catalogs and registration logic remain in `language-pack/`. Generate
and check the embedded resource snapshot with:

```sh
node scripts/build-builtin-locales.mjs
node scripts/build-builtin-locales.mjs --check
node --test tests/builtin_locales.mjs
```

The checked-in snapshot in `locales/dsh/` avoids adding Node or network access to
Cargo builds. Rust includes the five resource files directly; no optional tarball,
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
responses are ignored. The dialog is not exposed as a webview command. Actual
owned-runtime launch wiring remains to be implemented.

**Activation is not implemented yet.** The embedding API by itself does not prove
that a linked release ELF retains the resources; owned-runtime lifecycle calls
and package/native integration tests must establish that before completion.

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
