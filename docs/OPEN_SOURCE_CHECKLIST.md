# Open-source publication checklist

Source publication and binary distribution have different requirements. This
checklist is not publication approval or evidence of a successful public CI run.

## First public snapshot

This is the first public snapshot of work developed privately, not a claim that
all development began on the first public day. The public repository starts with
one new root commit using a noreply identity. No old Git history, private refs,
remote configuration or private CI logs are to be published.

Past internal source/secret audits and validation informed this snapshot. They
are historical evidence, not a scan of the final public root or public run links.
Verify the current public CI run's exact head and final conclusion in GitHub
Actions; historical validation is not a status guarantee. No scan proves the
absence of all secrets.

## Before publishing the snapshot

- [ ] **Review the exact exported tree and new root metadata.** Check source,
  configuration, fixtures, documentation and binary assets for credentials and
  personal metadata. Ensure no old Git history or private setup/account details
  have entered the export. Scan the final new root after all preparation changes.
- [ ] **Review public surfaces beyond Git:** issue/PR bodies and comments, Actions
  logs, releases/artifacts, wiki/discussions if enabled, screenshots and attachments.
  Automated source scanning does not cover these surfaces completely.
- [ ] **Review branding and attribution.** The original Ocean Ice master and its
  rendered derivatives replace DeepSeek icons in this snapshot. Verify the final
  assets and package contents against [BRANDING.md](BRANDING.md) and `NOTICE`.
  Preserve attribution for included third-party components; copyright licensing
  is not trademark permission.
- [ ] **Establish confidential reporting.** Confirm that GitHub private vulnerability
  reporting is actually enabled and tested, and publish only a real, approved
  private conduct contact. Do not invent an address or promise a response time.
- [ ] **Configure repository protection:** require PR review and passing `msrv`,
  `test` and `gitleaks` checks as appropriate; disallow force pushes/deletion of the
  default branch. Review admin bypass and fork workflow approval settings. These
  are hosting settings, not enabled by writing policy files.
- [ ] **Enable available GitHub security controls:** secret scanning/push protection
  and dependency alerts where the hosting plan supports them. CI scanning
  supplements those controls; it does not block a push before the secret reaches Git.
- [ ] **Verify the final public head:** maintainers must confirm fresh public CI,
  secret scanning and documentation links after all changes. Past internal green
  results do not establish success for the new public root.
- [ ] **Authorize publication explicitly.** A preparation checklist is not approval.

## Before distributing binaries

- [x] Collect reproducible default-feature Linux Cargo evidence and retain full
  notices/covered-source payloads in explicit package and initial-install paths.
  See [binary license audit](BINARY_LICENSE_AUDIT.md). This is not an exact linker
  inventory, license election or clearance of the remaining gates below.
- [ ] Inventory licenses/notices for all linked Rust and native dependencies and any
  bundled AppImage runtime, using the actual lockfile/target/build artifacts.
- [ ] Include the required texts/source offers/notices in each artifact, not just
  Ocean's MIT text. `NOTICE` is not a complete dependency-license inventory.
- [ ] Complete the packaging/distro acceptance and independent authenticity/signing
  steps in [RELEASING.md](RELEASING.md). Do not treat placeholder keys or checksums as
  a production trust root. No signed update feed or genuine AppImage acceptance is
  implied by source readiness.

## Repeating the secret audit

Use a trusted Gitleaks 8.30.1 or a deliberately reviewed update. CI is configured
to pin the x86_64 release archive digest and print fully redacted output without
uploading findings. Verify that configuration in the public snapshot. Scan the
new publication root, not private development refs:

```bash
gitleaks git --redact=100 --ignore-gitleaks-allow --log-opts=HEAD .
```

Review any result before adding an exception. Use only exact reviewed
fingerprints relevant to the public tree/history; do not carry private-history
exceptions into the new root or exclude entire test directories or generic rules.
Workflow changes still require human review. Dependabot tracks action and Cargo
updates, not the manually pinned scanner archive; update its version and digest
together after upstream verification.

No scan proves the absence of all secrets: arbitrary formats, encoded/encrypted
material, binary payloads/OCR and external profiles were not exhaustively
inspected. Never validate suspected credentials against a provider just to see
whether they work. If a real credential is found, revoke/rotate it first and
agree on cleanup; deletion alone is insufficient.
