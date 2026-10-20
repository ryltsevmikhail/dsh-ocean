# Security policy

## Reporting a vulnerability

**Do not put credentials, exploit details involving private data, or an active
vulnerability in a public issue, PR, workflow log or screenshot.**

Use [GitHub's private vulnerability reporting](https://github.com/ryltsevmikhail/dsh-ocean/security/advisories/new)
when the repository's **Security → Report a vulnerability** option is available.
This link is not a guarantee that reporting has been enabled. If it is unavailable,
open a content-free issue asking the maintainer to establish a private reporting
channel; do not include the vulnerability until that channel is agreed. No private
email address or guaranteed response time is currently advertised.

Include the affected version/commit, platform, relevant component, expected versus
observed behavior, impact and minimal reproduction using synthetic data. State
whether a fix or coordinated disclosure timeline is proposed. Do not send real
profile archives, private keys, API tokens, authenticated URLs or other users' data.

If a credential was exposed, revoke or rotate it with its issuing service first.
Deleting a file or rewriting Git history does not make a leaked credential safe.
Maintainers should assess forks, PR refs, artifacts, caches and release assets too.

## Supported versions

Ocean is pre-1.0. Security fixes currently target the latest `main` and may require
rebuilding from source; there is no maintained older-version branch or guaranteed
backport/support window. A version string or successful CI build does not establish
a signed or audited release. See [release verification](docs/RELEASE_VERIFICATION.md).

## Trust boundaries and limitations

- Ocean is an independent Linux Tauri/WebKitGTK shell, not a security sandbox for
  DSH, its plugins, tools or model-driven actions. Those components can have broad
  access under your user account. Configure their own permissions separately.
- Loopback is the default. Launch URLs and pairing QR codes contain bearer access;
  treat them as passwords. LAN exposure is opt-in, has no native TLS probe support,
  and must not be used on an untrusted network.
- The main webview is restricted to the attached origin; native shell/filesystem
  capability plugins are not granted to arbitrary remote content. This does not
  make an already compromised attached server trustworthy.
- Tokens are cached with private permissions and known launch-token patterns are
  redacted from diagnostics. Redaction is not comprehensive data-loss prevention:
  inspect all diagnostics before sharing, including paths and application logs.
- Backups can contain secrets. Configuration snapshots are not complete backups of
  global credentials, sessions or storage. Runtime rollback does not roll back data
  migrations. Keep separately verified backups before maintenance.
- The project downloads upstream runtime dependencies during explicit bootstrap.
  Checksums detect mismatch but do not independently establish publisher identity.
  Review the runtime manifest and third-party licenses. No production Ocean signing
  key, signed update feed or automatic shell updater is supplied.
- Process-group cleanup does not contain descendants that deliberately escape the
  group. Concurrent restart operations are a known follow-up, not an isolation guarantee.

See [architecture](docs/ARCHITECTURE.md), [downloads](docs/DOWNLOADS.md) and
[update testing](docs/UPDATE_TESTING.md) for precise contracts and acceptance limits.

## Maintainer handling

Acknowledge privately when possible, reproduce in isolation, assess affected
versions and coordinate a fix/disclosure. Share only the minimum necessary data
with reviewers. Do not claim a vulnerability is fixed until the relevant behavior
has been tested. This volunteer project offers no response-time or remediation SLA.
