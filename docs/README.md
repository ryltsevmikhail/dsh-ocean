# Documentation

Start with the [project README](../README.md) for status, requirements and source installation.

## Users

- [Usage](USAGE.md): runtime selection, maintenance, rollback, pairing and CLI.
- [Portable installation](INSTALLATION.md): trust, new installs and ledger-based removal.
- [Profile backups](PROFILE_BACKUPS.md): scope, verification and offline restore.
- [Downloads](DOWNLOADS.md): save/cancel behavior and filesystem boundaries.
- [Notifications](NOTIFICATIONS.md): activity notices, privacy and limitations.
- [Shell updates](SHELL_UPDATES.md): conservative installation hints, not self-updating.
- [Support](../SUPPORT.md) and [security reporting](../SECURITY.md).

## Contributors and maintainers

- [Contributing](../CONTRIBUTING.md), [conduct](../CODE_OF_CONDUCT.md), [governance](../GOVERNANCE.md).
- [Architecture](ARCHITECTURE.md): lifecycle, authentication and failure modes.
- [Update testing](UPDATE_TESTING.md): isolated fixture, native and real-runtime acceptance.
- [Packaging](PACKAGING.md): tar/deb/AppImage builders and redistribution limitations.
- [Binary license evidence](BINARY_LICENSE_AUDIT.md): collected Rust notices/source and unresolved release gates.
- [Toolchain/native observations](TOOLCHAIN_NATIVE_AUDIT.md): ELF-bound reports, compiler source terms and provenance limits.
- [Release checklist](RELEASING.md) and [release verification](RELEASE_VERIFICATION.md).
- [Roadmap](ROADMAP.md) and [changelog](../CHANGELOG.md).
- [Open-source publication checklist](OPEN_SOURCE_CHECKLIST.md): audit scope and approval gates.

Read commands and their scope before running them: installer, updater, rollback,
restore and service tests can mutate state. An opt-in acceptance test is not a
routine instruction to run against a live profile. Historical validation evidence
is tied to the named commit/platform; it does not automatically cover future heads.
