# Profile configuration snapshots

This is a configuration recovery feature, **not a backup of all DSH data**.
The implementation is in [profile_backup.rs](../src/profile_backup.rs), with
[CLI guards](../src/profile_cli.rs) and native backup/verification tray actions.

## Scope and privacy

A snapshot contains the regular files and directories below
`$DSH_HOME/profiles/NAME` (default `~/.dsh/profiles/web`), except `.git` and
`node_modules` trees. Custom local files inside the profile are included.

It does **not** include global credentials, global patches, sessions, storage
ledgers, runtime binaries, or plugin sources located outside the profile.
Symlinks and hardlinked files outside excluded trees are refused, not followed.

Profile files can themselves contain secrets, including custom `.env` files or
inline configuration. Snapshots are private directories (`0700`) with regular
files stored as `0600`. They are not encrypted, signed, uploaded, or committed
to Git. SHA-256 detects corruption against the manifest; it does not establish
who created a backup. Restore only snapshots you trust.

## Create and verify

Use **Back up web configuration…** from the tray, or:

```bash
dsh-ocean --backup-profile web
dsh-ocean --verify-profile-backup /absolute/path/to/snapshot
```

The CLI prints a JSON summary, not configuration contents. Default storage is
`~/.local/state/dsh-ocean/profile-backups`; `XDG_STATE_HOME` is respected.
The snapshot root contains `manifest.json` and `data/`. The manifest records the
profile, scope, exclusions, file sizes/checksums and owner-executable flags.

Creation is read-only to profile files. It checks file identities/timestamps
before and after copying, rescans the tree, and refuses detected changes. Ocean
operations serialize on a profile lock. Other DSH writers do not honor that
lock, so **live snapshots are explicitly best-effort**, not an atomic database
or session-store backup. Stop writers for stronger consistency.

Limits: 10,000 entries, 16 MiB per file, 128 MiB total file data, 32 directory
levels, and a 4 MiB manifest. Filenames must be normalized relative UTF-8 paths;
profile names are at most 64 ASCII letters/digits, dots, underscores or dashes
and cannot start with a dot. Unsupported paths, types or limits fail clearly
rather than being silently omitted.

## Native verification (not restore)

**Verify configuration backup…** in the tray opens a local-only folder picker
with folder creation disabled. Cancel closes it without starting verification.
Choosing a snapshot runs the existing bounded structure/size/checksum verifier
on a blocking worker, not the GTK thread. A per-client one-operation RAII gate
covers selection and hashing; it releases on picker Cancel or actual worker
completion (success, failure or cancellation), never merely on a Cancel request.
A native progress window polls aggregate hashed bytes/checked-file counts at
250 ms without file paths, contents, percentages or per-chunk persistence.
This action does not create the backup root, automatically read the current DSH
profile/global credential stores, install dependencies, query/stop services or
call restore.
The selected snapshot is read-only; normal GTK preferences are separate from
snapshot/profile data.

The result is plain selectable native text with **Close only**, no Restore/Apply
control. It shows validated profile name, normalized timestamp metadata, counts
and total configuration bytes—not file contents, per-file paths, raw JSON or a
list of filenames/hashes. Invalid timestamp strings are not echoed. Failures
use coarse fixed text instead of forwarding path-bearing verifier errors.
The explicit CLI can provide local operator detail when needed; inspect output
for private metadata before sharing it.

A successful checksum pass is not signing/authenticity, a frozen snapshot,
proof that other writers stopped, or data-schema rollback. Profile-local files
can themselves contain secrets/databases; only stores outside the profile are
outside this backup's scope. Native restore remains unimplemented. Restore still
requires the offline CLI and independent writer shutdown below; this screen
cannot do it for you.

**Cancel verification** (or closing the running progress window) requests
cooperative cancellation. The window stays open and says **requested** until
verification returns; the worker is never forcibly killed and may wait for an
in-flight filesystem operation. The read-only verifier checks before stages,
between directory-enumeration/metadata entries, between 64 KiB manifest/hash
reads, after each checked file and during the final inventory rescan. Progress
uses static Opening/Inventory/Hashing/Rechecking stage labels and aggregate
counts, never entry names. Full hash byte/file counts are not success: final
identity and inventory checks still remain. Existing create/restore/CLI callers
use no-op cancellation controls; no cancel authority is added to restore.

Before returning a verified result, the verifier rescans the held data directory
and compares its complete inventory/stamps to the initial scan. It rechecks the
snapshot root entries and held/reopened manifest, data-root and snapshot-root
identities/metadata. If these final checks detect a changed earlier-hashed file,
extra entry, rewritten manifest or replacement, verification fails closed
without deleting or repairing anything. This stricter read-only preflight also applies to offline restore;
normal format-v1 snapshots remain compatible. Per-file descriptors and final
rescans are **not a filesystem transaction or freeze**. Concurrent changes after
the last check, malicious same-UID/filesystem behavior and arbitrary observer
callback side effects are outside that guarantee; reverify before any later use.
No signature, immutable snapshot or service/data-schema rollback is implied.

A shared mutex linearizes cancellation against result commitment. If a request
wins before commitment—even after the final hash—the result is Cancelled with
no successful checksum claim. If completion won first, Cancel cannot rewrite
it. A terminal result is consumed once; UI does not claim that every winning
cancel physically interrupted a read/checkpoint. The per-client gate stays
held until the worker really returns. Background-task failure yields fixed
failure text, never raw diagnostic contents. No snapshot/profile/service/package
mutation, cleanup or deletion is performed by cancellation.

[History window tests](../tests/profile_history_ui.rs) render synthetic
inventories through a pure formatter and drive the real GTK window under the
isolated display: selectable plain text, modal, title, Close-only responses and no
mutation of the snapshots. Fixture-based restore tests are serialized inside the test binary because they
exercise flock/rename-heavy paths; isolation is per test, ordering is not.
[Retention tests](../tests/profile_backup.rs) cover listing without hashing or
leaking paths, unchanged bytes/mtimes, per-profile pruning, unrelated-file and
other-profile preservation, refusal of `keep=0`/`keep>1000`/invalid profile names,
snapshot-shaped symlinks left un-followed, and checkpoint cancellation before any
deletion. [CLI tests](../tests/profile_cli.rs) cover the mandatory confirmation,
the content-free listing output and end-to-end pruning in a private sandbox.

[Native verification tests](../tests/profile_verify_ui.rs) use synthetic
snapshots to check valid/tampered inputs, content-free summaries, invalid-date
non-disclosure and unchanged snapshot/profile bytes/mtimes. A pure RAII test
checks concurrent one-worker ownership/release. Cooperative tests abort before
I/O, during metadata enumeration, bounded manifest reads, real hash chunks and
final inventory checking; they preserve data and verify aggregate monotonic
counters. Seven controlled mutation/replacement cases fire at the first
Rechecking checkpoint (before the final rescan), not during/after its last file
check. They prove observable rejection/preservation, not isolation of every
identity defense or atomic detection of all concurrent mutations. Serial and 32 concurrent cancel/commit races check
one-shot terminal semantics. An ignored real GTK test requests Cancel, verifies
the progress window stays open until worker completion (including window-close
requests), then drives picker Cancel/Accept, checks SelectFolder/local-only/
no-mkdir properties and modal parenting, verifies the summary has no Accept/
Apply, then closes it. Actual blocked-filesystem cancellation and the full
Tauri worker/timer/guard dispatch are not directly driven in those fixtures. CI runs it
only on the private XDG/non-activating D-Bus + Xvfb harness. CI repeats this fixture
in two further fresh private displays, without ignoring a failed first run.
After an observed eight-second picker watchdog failure, its driver navigates into
the target SelectFolder and waits for current-folder/filename readiness before
clicking the actual sensitive button, rather than treating one accepted async
set_filename request as readiness. The exact previous GTK load ordering was not
captured; timeout diagnostics now log phase/readiness booleans only, never paths.
The watchdog remains eight seconds; production picker behavior is unchanged.
Actual installed-client tray click/visual acceptance remains pending; tests never
use real user profiles.

## Data-downgrade gate for restore

Snapshots now record `clientVersion` and the detected `runtimeVersion` (both
optional for older manifests). Before restoring, the client compares the
snapshot's runtime version with the currently selected one: if the snapshot data
came from an **older** runtime, restore refuses with an explanation, because an
older profile layout may be schema-incompatible with the newer runtime that would
read it. Only an explicit `--allow-data-downgrade` performs that restore, and the
receipt then reports `dataDowngrade: true` together with a warning that schema
compatibility was *accepted by the operator and is not verified*.

Restoring data captured by a newer runtime into an older one is not treated as a
downgrade (it usually is an upgrade path) and does not require the flag. When
either version is unknown, missing or unparseable, no refusal is invented: the
restore proceeds and the receipt warns that compatibility could not be compared.

Limits: this is a **version comparison only**. It does not read, migrate or
validate DSH's data schema, does not downgrade a schema, does not run migration
scripts, and cannot know whether a third-party bundle changed the on-disk format.
Treat an accepted downgrade as a manual, deliberate risk decision.

## Exporting a snapshot as one archive

`dsh-ocean --export-backup SNAPSHOT DEST.tar` verifies the snapshot and writes a
single **plain ustar tar** containing `manifest.json` plus `data/...`. Fixed
metadata (mtime 0, uid/gid 0, no owner names, no PAX headers) makes the archive
byte-for-byte reproducible from the same snapshot, and an existing destination is
never overwritten. `--verify-export ARCHIVE.tar` re-reads it and re-checks every
file hash against the embedded manifest, so a truncated or edited archive is
refused.

Reading an archive is bounded: each member is limited to 1 GiB and the whole
archive to 8 GiB, enforced from the declared header sizes before any allocation,
so a hostile archive cannot make the client reserve gigabytes.

Limits: exports are **unsigned**. They prove integrity relative to the snapshot
manifest, not who produced them, and this client has no key or trust root to
sign with. Snapshots can contain conversation content or embedded credentials, so
treat an export exactly as private as the backup folder it came from.

Tray **Export a backup…** picks a snapshot folder, then a destination `.tar`, shows
the manifest-only file count and byte total, and exports on a blocking worker with
a **Cancel export** action and a one-run gate. It never overwrites an existing file
and never claims the archive is signed; the result dialog repeats both facts.

### Importing an export

`dsh-ocean --import-export ARCHIVE.tar DEST_DIR` extracts an export into a new
snapshot directory, re-checking every hash against the archive's embedded
manifest and then running the owning engine's own verification before publishing.
The destination must not exist; extraction happens in a private sibling staging
directory that is removed if anything fails, so a tampered archive leaves nothing
behind. Re-exporting an imported copy yields byte-identical bytes.

Limits: an import proves integrity against the manifest that travelled inside the
same archive, which is **not** proof of authorship. Treat imported snapshots as
untrusted until you have your own reason to trust their source.

## Verifying every snapshot at once

`dsh-ocean --verify-backups [DIR]` sweeps the backup root and verifies every
recognized configuration and whole-data snapshot name through its owning engine,
reporting a `verified` list and a `failed` list (kind + snapshot name). Enumeration
does not require a readable manifest: missing/broken manifests and symlinked
candidates are reported as failures, not silently excluded by retention filters.
The sweep is bounded to 200 candidates, read-only and never deletes or repairs
anything. Exit **0** means all candidates passed (including an existing empty
root); exit **1** means a candidate failed or the root could not be opened safely.
On per-snapshot failures the JSON lists are still printed. Missing, unreadable,
non-directory and symlinked roots fail instead of masquerading as empty roots.

## Engine limits and known gaps

The two engines keep separate caps on the same shared, no-follow scanners: the
configuration engine allows 16 MiB per file, 128 MiB and 10 000 entries, while
whole-data snapshots allow 512 MiB per file, 4 GiB and 200 000 entries. Retention
orders `data-*` snapshots by the creation time embedded in the name, not by the
whole name (a pid is not a clock).

A restore that fails midway reports which roots were already replaced and which
previous copies were retained. If the retain rename fails, the previous root is
renamed to a `.ocean-retained-fallback-*` name; if even that fails, the error names
the staging path and tells the operator to move it out. `.ocean-data-restore-*`
directories are never collected automatically (`collectable:false` in
`--list-backup-staging`) because they may hold the only copy of replaced data.

Manifest matching during verify/import is indexed, manifests are read without
following a symlink and are size-bounded before allocation, and the staging
collector deletes through held descriptors without following links.

Every copied file and every recorded directory is fsynced before the snapshot is
published, and the manifest carries `consistent`: a re-walk of the live tree after
copying sets it to `false` when sessions or stores changed while the copy ran, and
the tray result then says the snapshot is a best-effort point-in-time copy and
recommends stopping writers. `--backup-data` prints the same flag in its receipt.

The directory that receives an archive or an imported snapshot is fsynced as well,
so a power loss right after a successful command cannot lose the new entry.

## Abandoned staging copies

An interrupted snapshot (`SIGKILL`, power loss, full disk) can leave its staging
directory behind. Those directories are always named with one of the generated
prefixes `.ocean-stage-`, `.ocean-data-stage-` or `.ocean-data-restore-`.

`dsh-ocean --list-backup-staging [DIR]` lists them (generated name, bytes, age)
without deleting anything; `--collect-backup-staging --older-than-minutes N
--confirm-collect [DIR]` removes only entries with those prefixes that are older
than `N` minutes and are real directories. Symlinks are never listed or followed,
a directory containing an entry this tool will not delete is left in place and
reported as kept, and published snapshots, backups and live data are never
touched. DIR defaults to the configured snapshot root; the DSH home is also
checked because restores stage there.

Limits: age comes from the directory mtime, so a clock change or a
deliberately-touched directory can delay or hasten collection. This is not a
general garbage collector for the backup folder.

## Whole-data snapshots (sessions and shared stores)

Tray **Back up sessions and stores…** asks first with the exact counts/bytes that
will be copied (plus a free-space warning), runs the copy on a blocking worker so
the GTK thread stays responsive, shows a count-only progress dialog refreshed every
200 ms with a **Cancel snapshot** action, refuses a second concurrent run, and
reports only the resulting file count/bytes — never a path or file name. Cancellation is cooperative: the copy stops after the current file, the staging
copy is removed and live data is untouched (no GTK object crosses the worker
thread; the worker only updates shared counters). The copy is read-only with
respect to live data; restoring stays an explicit offline CLI command.

`dsh-ocean --backup-data [DIR]` copies the two data roots of the DSH home —
`sessions/` and `storages/` — into a separate, explicit snapshot format with scope
`dsh-data`:

```
DIR/data-<stamp>/manifest.json
DIR/data-<stamp>/data/sessions/...
DIR/data-<stamp>/data/storages/...
```

Defaults: `DIR` is the configured snapshot root (created private if missing).
Limits are larger than the configuration engine (512 MiB per file, 4 GiB total,
200 000 entries, 8 MiB manifest) because sessions legitimately grow. Paths in the
manifest are always prefixed with their root, so a tampered manifest cannot point
outside `sessions/` or `storages/`. Files are copied through held no-follow
descriptors and checksummed; symlinks, special files and non-UTF8 names are
**skipped and counted** in `skipped` rather than followed, so a partial copy is
never silently presented as complete. Credentials at the DSH home root are never
part of this snapshot.

`--verify-data-backup SNAPSHOT` re-reads structure, sizes, checksums, both root
identities and a final inventory comparison; `--prune-data-backups --keep N
--confirm-prune [DIR]` deletes older `data-*` snapshots, verifies each manifest
before deleting, and leaves unreadable ones for inspection.

`--restore-data SNAPSHOT --confirm-offline` replaces the roots that the snapshot
contains (roots absent from the snapshot are left untouched, never deleted). It
requires the same offline assertion as profile restore: the client re-checks that
the managed service is inactive, no Ocean socket is listening and the configured
origin does not answer, both **before staging** and again **immediately before each
atomic exchange**. Every file is re-verified against the manifest while staging,
then the root is swapped with `RENAME_EXCHANGE` (or `RENAME_NOREPLACE` when the
root did not exist), and the previous root is renamed to
`<root>.ocean-retained-<stamp>` inside the DSH home — nothing is deleted, so a
manual rollback stays possible. Credentials at the home root are never touched.

Limits: this is a version-agnostic, non-migrating copy-back. Session data may be
schema-incompatible with the selected DSH runtime, entries the snapshot skipped
(symlinks/special files) are simply absent afterwards, and the retained previous
root is not restored automatically. An interruption between exchanging two roots
can leave one root restored and the other not; both previous roots remain on disk.
Restoring while any writer is still running is unsupported and cannot be detected
beyond the checks above. Snapshots contain conversation/log data
and may contain credentials embedded in them; keep the backup root private. Deleted
snapshots are unrecoverable, and checksums are not signatures.

## Backup inventory and retention

`dsh-ocean --list-profile-backups` prints a read-only, content-free inventory of
the snapshot root: for every direct child whose name is exactly
`profile-<name>-<pid>-<nanos>` and whose manifest validates, it reports the
snapshot name, validated profile, normalized UTC timestamp (or nothing when the
timestamp is unparseable) and manifest file/directory/byte counts. It reads only
`manifest.json`; it never hashes or returns profile file paths, contents, hashes
or credentials. Symlinks, special files, unrelated names and snapshots whose
manifest profile disagrees with the name are skipped, never followed. A missing
backup root lists as empty.

`dsh-ocean --prune-profile-backups --keep N [--profile NAME] --confirm-prune`
deletes the oldest snapshots of each selected profile beyond the newest `N`
(`N` from 1 to 1000; `--confirm-prune` is mandatory). Deletion walks the snapshot
with held, no-follow directory descriptors: only real directories and
single-linked regular files are accepted, and any other entry — symlink, FIFO,
device, multiply-linked file — aborts that snapshot's removal and reports it in
`unreadable` instead of being followed or partially removed. Entries that fail to
list or validate are kept for the same reason. Other profiles, unrelated files in
the backup root, and the active DSH profile are untouched. A caller checkpoint
runs before each deletion so a future UI can cancel cooperatively.

Limits: pruning is not transactional — an interruption mid-walk can leave a
partially deleted snapshot directory, which then fails manifest validation and is
kept as `unreadable`. Deleted snapshots are unrecoverable; there is no archive,
external copy, checksum-authority or signature. Retention is explicit only: the
client never prunes on a schedule, on snapshot creation, or in the background.
Snapshot profile files may contain secrets; keep the backup root private.

**Backup history…** in the tray opens a read-only, modal window listing the same
inventory as `--list-profile-backups`, plus a separate section for whole-data
snapshots (`--list-data-backups`: count, validated time, file count, bytes — no
snapshot names, paths or contents): snapshot count, total configuration bytes,
and per-snapshot profile, validated time, file/directory/byte counts. It shows no
file paths, contents, hashes or credentials, and offers **Close only** — no
Restore, Verify, Prune or Delete control. Restore and retention stay explicit CLI
operations with their own confirmations; the window states that nothing was
changed. Failures use fixed text rather than forwarding backend diagnostics.
The window is informational and does not prove authenticity or a frozen snapshot.

## Restore in an offline maintenance window

Quit Ocean and stop **all** DSH CLI/server writers first. In particular,
`dsh-web.service` must be inactive. Stopping that service disconnects chats it
hosts: do this from a separate terminal, never through an agent running on it.

Then inspect the snapshot and explicitly request an offline restore:

```bash
dsh-ocean --verify-profile-backup /absolute/path/to/snapshot
dsh-ocean --restore-profile web /absolute/path/to/snapshot --confirm-offline
```

The requested name must match the manifest. Restore:

1. holds the same runtime lock used by the updater and a profile-operation lock;
2. refuses an active user unit, an active Ocean socket, or HTTP at the configured
   local origin (a stale socket file alone does not block it);
3. verifies the manifest, every file checksum, sizes, paths, and the exact data
   inventory; no symlink or archive traversal is accepted;
4. builds a fresh private profile beside the target without executing scripts;
5. repeats offline checks and refuses detected target edits/replacement;
6. atomically exchanges the complete directory and retains the replaced
   profile, including its old dependencies/cache, under `ocean-retained-*`.

The JSON receipt gives the retained path and any warnings. The new profile is
private; executable files get owner-execute only. No dependency installation,
lifecycle scripts, service restart or credential changes are performed.
Review manifests before reinstalling missing dependencies. Retained profiles
are for manual recovery; a restore-history/undo UI is not implemented yet.

Atomic exchange requires Linux `renameat2` support from the filesystem. There is
no unsafe two-rename fallback. If activation fails, the target is left intact.
After activation the old directory is never deleted, even if naming it or
syncing the parent fails; such conditions appear in the receipt.

These guards cannot prove that an unrelated CLI writer has stopped, or prevent
an operator from manually starting one during the maintenance window. The
`--confirm-offline` flag is an operator assertion, not a global DSH writer lock.

## Explicitly not covered yet

- Global/session/storage backup, coordinated database snapshots, or data-schema
  migration rollback during updates.
- Automatic profile restoration by the updater.
- Signed/encrypted backups, archive import/export, retention/garbage collection,
  or a native restore/migration wizard.
- Power-loss/filesystem-corruption guarantees.

Snapshot and restore failure paths are tested only with disposable fixture
profiles. Tests never read the user's credentials or stop the live server.
The tray action is compiled but its manual click-through acceptance test is
still pending.
