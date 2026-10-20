# Project governance

DSH Ocean is a small maintainer-led project. Repository administrators maintain
access and release infrastructure; contributors propose changes through issues and
pull requests. It is not a foundation, vendor-supported product or democratic
membership organization.

## Decisions

Discuss substantial design changes in an issue before implementation. Maintainers
consider user impact, security boundaries, maintenance cost and compatibility with
upstream DSH. Record significant decisions and trade-offs in the relevant docs or
PR. A contribution may be declined even if technically correct; explain why and
avoid leaving ambiguous promises of support.

Maintainers review and merge changes. Passing CI is necessary evidence, not a
substitute for review, security judgment or third-party licensing checks. Bot/AI
reviews must be identified as such and cannot impersonate a human approval.
Contributors should not merge their own sensitive changes without an independent
review where one is available. The current small team cannot guarantee two-person
review for every change; do not claim it as an enforced control.

## Access and releases

Grant the least repository permission needed, protect credentials and remove
access that is no longer required. All ongoing changes, including documentation
and automation, use a focused review branch and pull request, never a direct push
to `main`. Each PR states its purpose, tests, risks and deliberate non-goals.
Green CI or opening a PR does not authorize an agent to merge it: merging requires
an explicit maintainer decision. Keep the default branch protected; actual GitHub
settings must be checked separately from this policy, which does not enable them.

Only maintainers authorize versions, release assets, signing keys and publication.
Follow [RELEASING](docs/RELEASING.md). Never present local unsigned artifacts as
verified public releases or silently install new trust roots.

## Continuity

There is no guaranteed support window, release cadence or maintainer succession
process. If maintenance becomes inactive, the MIT license permits forks subject
to its terms and applicable third-party notices. A fork must not imply endorsement
or ownership of upstream trademarks. Changes to governance should be proposed in
public once the repository is public and reviewed like other project changes.
