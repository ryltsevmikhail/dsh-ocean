# Main Harness desktop language pack

A real local DSH bundle for **everyday main Harness desktop, chat and settings**, not an Ocean shell replacement. Languages are registered in this order: Русский (`ru`), Español (`es`), Português (Brasil) (`pt-BR`), Deutsch (`de`), Français (`fr`), 日本語 (`ja`), 한국어 (`ko`). Select a language with the existing **Settings → General → Language** control. The bundle never selects one on your behalf.

## Compatibility and scope

Tested target: npm-installed `@deepseek-ai/dsh` **0.2.0-rc.2**, `@deepseek-ai/dsh-client-locale` **0.2.0-rc.2**, Cordis **4.0.4**. Exact locale peer and `engines.dsh` record this target; engines are informational and do not replace the runtime's peer checks. There are no installed dependencies, install/prepare hooks, model tools, network calls, DOM replacements, monkeypatches, or private copies of React/Cordis.

Canonical English dictionaries were extracted from this runtime's compiled official client artifacts, not reconstructed from screenshots. Package versions, artifact SHA-256 hashes, variable names and key counts are recorded in `data/inventory.json`. Upstream English catalogs and derived translations retain the DeepSeek MIT notice in `LICENSE`.

| Namespace | Keys | Surface |
| --- | ---: | --- |
| `common` | 41 | Shared controls, code/JSON copying, Markdown labels |
| `settings.locale` | 1 | Existing Language row |
| `settings` | 34 | Settings shell, General, desktop update copy |
| `chat` | 186 | Main transcript, history, work details and usage |
| `conversation` | 369 | Composer, attachments, session/goal/todo detail UI |
| `workspace` | 112 | Workspace/session navigation and management |
| `model` | 23 | Model selector |
| `settings.models` | 113 | Model configuration and credential dialogs |
| `settings.plugins` | 5 | Plugins section |
| `settings.pluginInventory` | 40 | Plugin inventory |
| `pluginManager` | 189 | Official plugin manager UI |
| `sidebar` | 5 | New Session, sidebar toggles and global-panels accessibility labels |
| `settings.agentPreset` | 41 | Working-mode picker, preset settings, complete built-in mode guides |
| `settings.theme` | 9 | Appearance Light/Dark/System and conversation font-size controls |
| `shortcuts` | 56 | Keyboard shortcuts settings, editor, validation, conflicts and read-only failures |
| `settings.permission` | 12 | General default permission mode and full-access risk copy for new sessions |
| `permission.access` | 17 | Current-session access picker, full-access and experimental auto-review risk copy |
| `approval` | 5 | Approval waiting/details/escalation, Reject and Allow once |
| **Each language** | **1,258** | **18 complete dictionaries** |

Version **0.4.0** adds complete permission and approval catalogs: 34 keys per language (238 new entries). All 1,224 prior values per language (8,568 total) remain byte-equivalent by namespace, guarded by hashes captured from the actual preserved 0.3.0 archive before edits. The 0.1.0 and 0.2.0 baseline guards remain. The full package contains **8,806 entries** across seven languages.

**Safety:** this bundle only registers strings; its host function is a no-op. It does not change permission policy, sandbox mode, verdict/action identifiers, tool permissions, allowlists, configuration capabilities, paths or task state. General defaults apply to new sessions, unlike the current-session access control. Full access is not a sandbox guarantee. Experimental Auto review explicitly uses no sandbox and the same model as the agent before every native/PTC inner call; denied calls require human decisions, false allows/denials and extra tokens remain possible. The current approval owner offers Reject (`rejected`) and Allow once (`allowed-once`), not a reusable/persistent/session-wide approval action.

All dictionary entries in this scope are included for every language; placeholders retain their exact names and multiplicity. These catalogs use plain strings with `{name}` substitution: there are no function values or ICU plural programs to convert. Existing `.one`/`.other` keys and caller behavior are retained. Technical literals (protocol names, URLs, command identifiers, units, punctuation, format-only templates), and words whose spelling is legitimately shared, can remain identical to English; `data/unchanged.json` records them for explicit audit, not hidden fallback.

### Deliberate remaining gaps

- Other official feature namespaces are **not included**: browser/files/terminal, account/sign-in, shell/search/subagent settings, schedules, workflow and experimental voice/team features. Consequently other unowned feature labels can still be English. This is a scoped language pack, not complete product localization. A `common` fallback can translate shared labels but is not a promise to translate those features.
- Built-in working-mode names/descriptions are translated through `settings.agentPreset`; custom preset names and published `LocalizedText` overrides remain metadata-owned. The picker is gated by the official General → Show coding view preference (internally Developer tools). This pack does not change that gate, select a mode, alter permissions, or switch an existing task’s composition. The short upstream PTC description says it includes all Standard capabilities, while its full guide explicitly says the workflow tool is disabled; both upstream statements are translated faithfully rather than silently rewriting product copy.
- The shortcuts editor gets command labels from the public shortcut registry (`useCatalog` / `useFixedCatalog`); it renders `row.label`, not arbitrary namespace lookups. This scope translates its own three fixed menu actions and Open keyboard shortcuts command, groups, editor/accessibility/validation/read-only copy. Other packages own their registered command names, aliases and descriptions; unowned namespaces or hardcoded metadata can remain English, and activation-captured labels may need reload. This is not full shortcut-metadata localization. Physical key tokens, bindings and storage paths are unchanged.
- Native Ocean menus, setup/recovery screens and its own shell strings are outside this package. The previous shell localization worktree is independent and unchanged.
- A language pack can only supply registered namespace keys. Hardcoded English/Chinese strings, raw Host errors, schema/package metadata not routed through those dictionaries, and other plugins' own labels are not rewritten. Installed package titles/descriptions use separate `LocalizedText` metadata, not namespace lookup; third-party plugin localization is not promised.
- User/assistant messages, prompts, command names and arguments, paths, model/provider IDs, and tool result data are never translated or mutated. Some dictionary entries describe these items; only their surrounding UI copy is translated.
- Text captured once in an official registry (for example a command description registered at activation) may keep its previous language until that official component re-registers or the page reloads. Ordinary slot-rendered labels update immediately.
- The runtime owns browser-language matching, locale persistence and `<html lang>`. The pack does not add locale-dependent plural-selection logic, number/date formatting engines, fonts or bidirectional layout.
- Missing/new keys after a runtime upgrade fall back to English through the public API. Re-extract and review the inventory before widening the tested version. Other packs cannot own the same language id or `(namespace, language)` registration simultaneously; disable the conflicting pack first.

## Build and test (maintainers)

Run from this directory, with Node.js 22+ and npm:

```sh
npm test
npm run build
npm pack --ignore-scripts --pack-destination artifacts
npm run verify:fixture
VERIFY_FRESH_BROWSER=1 npm run verify:fixture
```

No dependency install is needed. Source: `src/register.mjs`; maintained catalogs: `data/<language>.json`; tests: `test/*.test.mjs`; generated runtime artifacts: `lib/index.js` and `lib/client.js`. The browser build uses the installed official decoration template's `window.__ModuleLoader__.load({id, factory})` convention. `dsh.client.inject` names the locale package; plugin `inject: ['locale']` names its service. Every language and namespace disposer is owned by `ctx.effect`, so unloading removes registrations and permits re-enabling.

`npm test` covers full key/type/placeholder parity, translation coverage, language order, reversible effects, manifest/script safety, the actual installed `LocaleRuntime`, lazy module loading, and fixture isolation fences. During sequential translation development, `LANGUAGES=ru,es npm test` checks only completed catalogs plus the entire infrastructure suite. The unfiltered default checks all seven.

`npm run extract -- /absolute/npm/runtime/prefix` refreshes the canonical English snapshot and provenance from an installed runtime. This is an explicit maintainer action and does **not** auto-translate newly added keys. English extraction evaluates only the isolated known dictionary literal, never an entire upstream plugin.

### Isolated packed-tarball verification

`npm run verify:fixture` requires built artifacts, the installed runtime, `pnpm` on PATH and `/usr/bin/chromium` (override with `CHROMIUM`). Override the read-only runtime prefix using `DSH_INSTALL`. It creates a fresh private HOME/DSH_HOME/profile/cache tree under `artifacts/language-pack-fixture-*`, packs the real tarball, installs through the real `dsh plugin ... add` flow with scripts disabled, checks `--dump-config`, and starts that fixture with `--host 127.0.0.1 --port 0 --no-open`. It never uses `~/.dsh/profiles/web`, the running 3080 service, or the user's browser profile.

The verifier checks a fresh install and an upgrade from the preserved 0.3.0 tarball (pinned SHA-256), retaining the 0.1.0 and 0.2.0 archive hash guards, then checks the real HTTP boot graph and served client bytes. Its own incognito headless Chromium context selects all seven languages through the existing settings UI. For each it cycles Light/Dark/System and restores the fixture preference, changes font size by one step and restores it, checks the font hint and accessible control names, opens/searches the shortcut reference and inline editor without pressing a chord, committing a binding, or invoking any shortcut, and verifies reset-all is disabled for unchanged bindings. Opening the inline editor activates the owner's recording lifecycle; the verifier dismisses it by pointer without pressing keys or committing a binding. It asserts the fixture's keybindings storage is unchanged. Reset confirmation and validation/failure states are not induced in the browser; their full copy is tested by exact resolution through the actual LocaleRuntime. It also checks New Session text and ARIA, global-panels ARIA, sidebar collapse/expand, the working-mode picker, available preset cards, Standard guide details/usage, model controls, composer, and locale persistence after a full reload. It captures screenshots and stops its owned process groups. It temporarily enables and restores Show coding view only in the fixture; it never starts a task, selects a new default, submits a prompt, enters credentials, or invokes a model/tool. The default browser run uses the upgraded profile; `VERIFY_FRESH_BROWSER=1` repeats all seven languages against the fresh-installed profile in a separate fixture. Results: `artifacts/fixture-result.json`, `artifacts/fixture-result-fresh.json` and per-fixture logs/screenshots. The temporary profile persists only as evidence; it is excluded from the distributable. A missing-default-workspace toast can appear in this unconfigured fixture; the picker and settings remain inspectable without creating a workspace. This is not installation into your live profile and does not update the current GUI.

The fixture exercises the available built-in preset roster dynamically; custom metadata overrides and the read-only label on a running task are not end-to-end tested (no task is created). All 34 new permission/approval keys are checked by exact value and placeholder resolution through the actual installed `LocaleRuntime`. The actual General permission menu is opened/read/closed without selecting any option in every language; complete advertised names are checked for clipping and the default label is checked unchanged after closing and reloading. Read-only replays of the page's observed official settings-describe and permission-catalog RPCs compare the authoritative `permission` namespace value, catalog default and effective default before/after each menu inspection and after reload; unrelated theme/locale revisions are excluded. The complete profile patch is not asserted unchanged, since intended locale/theme/welcome preferences also persist there; the authoritative permission-only snapshot avoids conflating those changes with policy. A passive HTTP JSON-RPC audit observes zero requests matching its selected forbidden method patterns (permission setting writes and approval/task prompt/command method names). This is not a universal audit of approval response packets or other transports. No real approval request or task is created or answered by the read-only fixture flow; the audit does not intercept or change policy. The actual composed Web fixture advertises all three labels: read-only, workspace-write and full-access; all are inspected without selection. Current-session access/risk dialogs, pending approval waiting/detail/action states and approval keyboard handling are **not browser-exercised**: no task or real request is created, and no approval verdict is sent. Official source bindings are separately verified, including Reject→`rejected`, Allow once→`allowed-once`, Enter/Escape outcomes, preset wire identifiers and the explicit full-access confirmation gate. This is not a claim of live approval/backend or risk-dialog visual acceptance. No supported standalone official presentation fixture was identified; no fake product DOM or permission monkeypatch is used.

Host-authored permission descriptions, custom preset labels, raw reasons and detail/call arguments remain data/schema-owned and may stay English. No DOM walk translates them. The official settings RiskConfirmation calls `t('close')`, absent from its 12-key owned settings catalog; this upstream missing-key boundary is not concealed with a partial alias. Namespace disposal and re-enable are also tested against that runtime without duplicate locales. No Ocean/Rust source is changed, so this stage runs the full package test suite rather than repeating unrelated root Cargo tests.

## Live installation is intentionally not performed

The source package is maintained in the Ocean repository. CI installs a pinned runtime into a disposable HOME and runs all package tests; shell release candidates build the optional tarball and checksum metadata from these sources. Dry-run releases validate the asset without uploading it or accessing production keys. It remains separate from signed shell/runtime updater manifests and is never auto-installed or enabled. Historical 0.1.0–0.3.0 tarballs are immutable test fixtures, not current release assets. The integration does not publish a release, replace a service, or change the working profile. If separately authorized later, the official installation form is:

```sh
dsh plugin --profile <intended-profile> add /absolute/path/to/dsh-ocean-desktop-language-pack-0.4.0.tgz --ignore-scripts
```

A deliberate browser refresh may be required after installing the new client module. Uninstall with the official plugin manager; owned registrations are disposed without modifying messages or existing built-in dictionaries.
