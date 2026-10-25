# Publish the checked Linux x86_64 RC2

`prebuilt-rc.yml` consumes the accepted archives declared in
`rc2-prebuilt-lock.json`. It does not build replacement archives, fetch unpublished
DSH packages from npm, change updater trust, or activate an installed runtime.
Its workflow commit differs from the accepted product source commit in the lock.

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
