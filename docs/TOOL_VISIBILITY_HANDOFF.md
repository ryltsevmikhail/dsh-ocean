# New-session handoff: tool visibility CPU backport

## Request and completed work

The user requested diagnosis and repair of excessive local DSH CPU on a MacBook Air running Omarchy Linux. Linux on this machine is intentional; do not confuse it with a remote Linux host.

Confirmed root cause in dsh-plugin-tool-management 0.20.0: unconditional teardown in applyToolRestrictions erased appliedByAgent keys and produced a self-triggering tools/change loop. The browser runtime amplified this via schema rebuilding/deep copying. Minimal backport is installed and was activated with explicit user permission for one DSH restart. Observed CPU decreased from approximately 104.7% to 6.5%; V8 confirmed patched source loaded. Debugger port 9229 was closed. Session storage was not changed, and sessions were not deleted.

See [incident report and rollback](TOOL_VISIBILITY_BACKPORT.md) for evidence, caveats and commands.

## Repository boundaries

These files were transferred as a patch onto the public repository's own main history. No private/archive history or raw CPU profiles were published. The original local incident commit is e371e6f; it must NOT be pushed/cherry-picked with its ancestor history into public main.

The public working branch is `fix/tool-visibility-cpu-backport`. Review/publish through the normal PR process; do not force push or merge without authorization. The user authorized pushing this fix, not bypassing protected main.

## Recheck in the next session

1. Inspect current git state and remote identity. Read the incident report.
2. Run `node scripts/backports/tool-visibility.cjs --check` to verify installed content (default web profile under the current user's home).
3. Run `node --test scripts/backports/tool-visibility.test.cjs scripts/backports/tool-visibility-installer.test.cjs` (8 tests; requires the supported installed module or VISIBILITY_FILE).
4. Recheck current service PID/CPU if needed; recorded PID 70035 is historical, not an assumption.
5. After package reinstall, explicit `--apply` is available only for the exact expected 0.20.0 source hash. No automatic reapplication hook is installed. Newer versions require upstream review, not bypassing the guard.

No pending runtime repair remains. Future work is PR/CI review and retiring the local backport once an upstream package includes the fix. Do not restart DSH or open an inspector solely to resume this handoff. A future restart/activation still requires coordination.
