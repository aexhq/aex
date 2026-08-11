---
title: public repository backlog
description: Deferred public-repository work with explicit revisit triggers.
keywords:
  - backlog
  - ci
  - supply chain
audience: maintainers and implementation agents
status: accepted
related:
  - references/develop.md
  - release/README.md
---

# Public repository backlog

## GitHub browser sign-in

Launch browser sign-in is Google-only. GitHub login is deferred rather than
shipped dormant: `github` is not a public `IdentityProvider`, the dashboard does
not advertise or accept it, and neither central deployable loads a GitHub OAuth
credential. Repository hosting, Actions, release provenance, and other GitHub
integration are unrelated and remain supported.

Revisit this when customer demand justifies a second login provider and separate
dev and production GitHub OAuth Apps are ready with the exact dashboard callback
for each plane. Reimplementation is a coordinated contract and deployment
change: restore the provider in the identity schema and generated Rust and
TypeScript wire models; widen the identity-domain and database provider
constraints; add the dashboard authorization option and provider-specific start
parameters; restore the server-side code redemption and verified-primary-email
profile read; add one GitHub client secret binding, capability declaration, IAM
grant, and startup probe to both central deployables; bind only the public client
ID in each dashboard environment; and restore focused unit, composition, and
callback tests. Do not add only the button or only the secret: the provider is
available when that whole vertical slice is deployable in both planes.

## Asynchronous supply-chain assurance

Dependency audits, licence inventory, packaged-artifact SBOM generation, and
vulnerability scanning are deferred during the startup phase. They must not
block pull requests, protected-main publication, dev deployment, or live tests;
scanner availability and newly published provider advisories are external state
that can change after deployment.

Revisit this when a customer or compliance commitment requires the evidence,
or when the team has enough CI capacity to run it asynchronously without
lengthening release feedback. The future lane should record findings for
triage, keep scanner failures non-blocking for dev, and introduce a blocking
rule only for a separately reviewed, demonstrably exploitable runtime-critical
finding.

## Browser MicroVM variants

The `2gb-browser`, `4gb-browser`, and `8gb-browser` image variants are excluded
from the public release authority during prelaunch. The pinned AL2023 ARM64
repository does not publish Chromium, so listing those variants would create
artifacts that deterministically fail in the provider image builder. The five
non-browser variants remain published.

Revisit this when the image build has a digest-pinned ARM64 browser artifact
with verified provenance and a live AWS MicroVM qualification. Keep the browser
runtime contract and local generator code until then; restoring release rows
requires the provider build and browser smoke to pass for every restored shape.

## Narrowed database logins for `central-api`

`services/central-api` was built to require four distinct Aurora logins over the
one cluster — `aex_authz` read-only for admission, plus `aex_control_api`,
`aex_identity_api` and `aex_finance_api` — with a start-up refusal if two of
them named the same secret. It now connects as the cluster's RDS-managed master,
which is what `central-control-api`, `central-identity-api` and `finance-api`
each already do, so the merged service is no different from its parts here.
`migrations/central/grants.toml` still defines the roles and their grant matrix;
what is deferred is *using* them from this deployable.

Dropped because the four `PostgreSQL` users exist in no plane. Every central
Lambda connects as the master today, so requiring them meant an operator
creating four users by hand before the service could start at all — friction on
every deployment and every plane rebuild, in exchange for a threat that does not
bite prelaunch: one central service reaching another central service's schema,
in a single process, in a single account, with no third-party code in it.

**This is not tenant isolation and does not weaken it.** Separate logins defend
service against service — a compromised billing handler being unable to read
identity's tables. Keeping one customer out of another customer's rows is a
different property, enforced in the application: the central edge resolves every
path-addressed resource from its own row and decides against the organization
that row names, never against an id taken from the request path
(`crates/aex-central-http/src/target.rs`,
`crates/aex-control-domain/src/authz.rs`). No database login has ever carried
that property, so nothing here relaxes it, and it must stay enforced.

Revisit this when any of the following is true: a central deployable runs code
the platform does not own, or handles a third-party payload, in the same process
as a schema it should not read; a compliance or customer commitment requires
demonstrable least privilege at the database; or a plane's roles are created by
migration rather than by hand, at which point the cost of the split is a
Terraform secret per role instead of an operator's checklist. Restoring it is
one secret ARN per login in `config.rs`, one Data API client per login in
`main.rs`, and the distinctness refusal that went with them.
