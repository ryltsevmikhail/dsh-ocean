# Contributing to DSH Ocean

Thank you for helping improve the Linux desktop client for DeepSeek Harness.
Read the [code of conduct](CODE_OF_CONDUCT.md) before participating. For potential
vulnerabilities, follow [SECURITY.md](SECURITY.md), not a public issue.

## Before you start

- Search existing [issues](https://github.com/ryltsevmikhail/dsh-ocean/issues) and
  [pull requests](https://github.com/ryltsevmikhail/dsh-ocean/pulls).
- Open an issue before a large change, new dependency, security-boundary change or
  platform port. Small fixes and documentation improvements can go straight to a PR.
- Keep Ocean a desktop shell: do not duplicate the upstream chat interface or
  silently modify a user's DSH installation, profile or services.
- Describe observed behavior and environment without posting tokens, authenticated
  URLs, cookies, profile contents, private prompts or unredacted journals.

## Development environment

Use Linux, Rust **1.90 or newer**, GTK 3, WebKitGTK 4.1 and Ayatana AppIndicator.
Python 3.11+ and Node.js are used by packaging/test helpers, not required to execute
the compiled shell itself. Follow [README](README.md) for native build dependencies.

```bash
git clone https://github.com/ryltsevmikhail/dsh-ocean.git
cd dsh-ocean
git switch -c fix/short-description
cargo build --locked
```

Do not run `cargo run` or installer/service/update scripts casually on a production
profile: launching the application can discover/start a user service and use your
existing DSH data. Use the private fixtures in [testing documentation](docs/UPDATE_TESTING.md).
Do not run integration tests as root. Do not disable sandboxing outside disposable
CI fixtures merely to make a test pass.

## Required checks

```bash
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
python3 -B -m unittest discover -s tests -p 'test_*.py'
node --test tests/*.mjs
git diff --check
```

Native GUI tests need a graphical environment; CI supplies private Xvfb and D-Bus.
Some network/service/real-runtime acceptance tests are deliberately ignored and
require explicit setup. Never run all ignored tests indiscriminately: dedicated
controller/worker tests may only be safe through their documented entry point.
Report which checks you ran and what you could not run. A mock passing is not real
DSH/provider acceptance. See the workflow and testing guide for authoritative commands.

## Pull requests

1. Keep the change focused and explain the problem, approach and user-visible effect.
2. Add a regression test for behavior changes, including failure/cleanup cases.
3. Preserve the declared MSRV, locked dependency resolution and offline fixture isolation.
4. Update relevant docs and `CHANGELOG.md` for user-facing changes. Do not bump the
   version, regenerate unrelated assets or reformat unrelated code without agreement.
5. Include test results and remaining limits. Screenshots must contain synthetic
   data only and are optional, not a reason to expose a real profile.
6. Wait for CI and maintainer review. Do not bypass branch protection or force-push
   shared branches; coordinate before rewriting a review branch.

Changes to authentication, navigation, subprocess ownership, path validation,
backup/restore, updater or installer logic need explicit failure-mode review.
Avoid shell interpolation of untrusted paths. Do not add real credentials even to
tests; use clearly synthetic values and disposable directories.

AI-assisted contributions are welcome if you understand and verify them. Describe
significant generated code and the verification performed; agent review is not a
substitute for accountable maintainer review. You are responsible for provenance,
license compatibility, correctness and privacy of your contribution.

## Licensing

By submitting a contribution, you agree that your original contribution is
provided under this project's [MIT license](LICENSE). Keep existing copyright and
license notices. Identify any third-party code, asset or patch and its source and
license. Do not submit material you lack permission to redistribute. No CLA or
DCO sign-off is currently required.

## Review and releases

The repository maintainer decides scope and merges, with CI as evidence rather
than a guarantee of correctness. See [GOVERNANCE.md](GOVERNANCE.md). Releases and
signing are maintainer tasks; follow [the release checklist](docs/RELEASING.md).
Contributing or merging a PR does not imply a support SLA or release date.
