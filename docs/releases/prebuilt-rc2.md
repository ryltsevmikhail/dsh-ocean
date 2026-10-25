# Publish the checked Linux x86_64 RC2

`prebuilt-rc.yml` consumes the accepted archives declared in
`rc2-prebuilt-lock.json`. It does not build replacement archives, fetch unpublished
DSH packages from npm, change updater trust, or activate an installed runtime.
Its workflow commit differs from the accepted product source commit in the lock.

The final runtime lock selects the documentation-complete archive
`ba86c35d76df38cadebd19b7b1be7a4db199c4ef05c311b748025be92ef6be31`
(487,101,871 bytes). All 39,970 previous regular files retain identical bytes and
executable flags; 796 added files contain component notices and source-access
instructions. Exact-archive relocation, native bindings and authenticated HTTP
401/303 were checked. The prior archive remains retained as evidence.

Publish these source companions on the runtime's same release page, without an
additional source-access charge. The runtime's retained notice instructions bind
their filenames and hashes; its updater signature covers the runtime archive,
not a separate signature on each companion:

| Source asset | SHA256 |
| --- | --- |
| `dsh-ocean-runtime-next-0.2.0-rc.2.ocean.2-1-x86_64-native-source.tar.gz` | `a39c6263aca9e46e5eda2162298dc6c2ac2c72359a5469b1f8e52d53f76617af` |
| `rc2-js-notice-source-companion.tar.gz` | `9b868660ae484cbf721b8f2263486b1be9e828c6c425a2ea628b8f883b993c6e` |
| `dsh-sdk-0.2.0-rc.2.ocean.2-473d474-source.tar.gz` | `6600cc88ad628aad7941fea0cd037915e18ea415ae92cdc175d25ddf9a733717` |
| `dsh-plugin-tool-management-0.20.1-ocean.1-1eb3a72-source.tar.gz` | `6b235c5ec754cc8d0e67663f13bd129ede1ccd4f486eb746223e5a0b3930059d` |

Sherpa/eSpeak's combined native component follows GPLv3-or-later source delivery;
its npm Apache declaration does not replace that component's terms. Other
components retain their own licenses. These are scoped source/notice findings,
not a claim of whole-runtime relicensing or byte-identical supplier rebuilds.

First integrate this workflow into the default branch so workflow_dispatch is
registered, and verify that its selected ref is allowed by the existing
release-shell/release-runtime environment policies. Preserve both environments'
existing keys and public fingerprints. Complete the native/notice review before
recording the existing `OCEAN_RELEASE_CLEARANCE` value; publishing authorization
alone does not supply that review.

Create the two locked release tags at the locked product source commit. Stage each
canonical archive into its own unpublished draft with that tag. For shell, also
upload the matching native report as `native-evidence.json`. Do not overwrite an
existing tag, published release or asset. Preserve previous versions and keys.
Any additional notice/source assets must be reviewed before staging; the signed
updater manifest covers the selected product archive only.

Run with dry_run=true first. This downloads the selected draft assets by ID and
validates source identity, full bytes and manifest construction without reading
signing secrets or uploading. A non-dry run additionally requires the protected
review gate, builds a separate verifier from the accepted source, signs with the
product's key, verifies the signature and artifact, and uploads public signature
metadata without --clobber. publish=true then publishes that captured draft ID as
a next prerelease, without setting latest.

The final step re-downloads all selected remote assets and compares them with the
verified local bytes. Recreated drafts, changed payload asset IDs and byte changes
are refused. Serialize staging with the same release-product-next concurrency
discipline and do not mutate drafts manually during publication: GitHub has no
atomic transaction combining asset reads with publication. If upload partially
fails, leave the draft for investigation instead of overwriting/retrying blindly.

Local tests cover both products' metadata layout and the actual publication Bash
step using a fake GitHub CLI, including asset/draft replacement. Local tests do
not establish a successful protected Actions run or signed client update.
