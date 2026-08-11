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

## Generic secrets and typed integrations

Launch has no generic secret vault and no first-class skill, tool bundle, custom
instruction, or MCP-server resource. Dedicated BYOK provider credentials remain
their own narrow write-only authority. Every other customer-supplied artifact is
an opaque registered workspace file that a session may mount; conventional
guidance such as `AGENTS.md`, skill files, tool bundles, and MCP configuration is
read through Bash inside the session rather than being interpreted as a separate
public capability.

Revisit generic secrets only when a concrete non-provider consumer cannot use an
opaque mounted file and has a complete custody, rotation, revocation, audit, and
least-privilege design. Revisit typed integrations only when the type provides a
customer-visible guarantee that an opaque file plus Bash cannot provide. Restore
each category as a whole vertical slice—schema, immutable resolution, retention,
credential authority where needed, runtime behavior, SDK/CLI, and user tests—not
as dormant routes or catalog labels.

## Hosted paid tools

The launch built-in model tool catalog contains Bash only. It executes inside the
session MicroVM and is charged as session compute. Hosted credential-backed tools
such as web search, their public executor deployable, and any user BYOK tool-key
surface are absent from the release rather than hidden behind an unreachable
catalog row.

Revisit this when customer demand justifies platform-managed paid tools and the
release can prove immutable tool identity, metering, permission and approval
binding, provider-key custody, result limits, and end-to-end reachability. Restore
the catalog row, Brain route, executor release unit, readiness and environment
bindings, and live receipts together.

## Session persistence and crash recovery

Launch sessions retain one provider generation for at most eight hours. Idle
compute may suspend and resume that same generation, but there is no public
persisted-session workspace and no S3-backed Brain or runtime snapshot. Explicit
termination or runtime loss destroys ephemeral compute and live files; runtime
loss terminates the session rather than reconstructing an ambiguous partial turn.
Durable registered workspace files and telemetry storage are independent and
remain supported.

Revisit crash recovery when measured runtime-loss frequency or customer
reliability commitments justify a new design. It must define the exact durable
turn boundary, provider-effect replay semantics, encrypted snapshot ownership,
retention/deletion, version compatibility, and recovery user experience before
adding storage or a resume path. Revisit persistent session files separately only
if customers need a second durable file abstraction in addition to registered
workspace files.

## Message attachments

Launch message admission is text-only. Files are not silently converted to path
text and are not dropped; customers mount opaque registered files or upload live
session files and refer to them from text/Bash.

Revisit attachments when the product has one immutable file-resolution and
retention contract, canonical message-block representation, provider rendering,
size limits, deletion behavior, and SDK/CLI upload workflow. Add all of those with
black-box tests before widening `MessageSendRequest`.

## Session trash and restore

Launch deletion is asynchronous and irreversible. It terminates the exact
generation, removes session-scoped user content and telemetry, and retains only a
minimal tombstone plus aggregate billing/audit facts. There is no recovery window
and no trash or restore route.

Revisit a recovery window only when customer demand outweighs the additional
retention and privacy complexity. A future design must pin recovery duration,
storage cost, delete/export interactions, account-pause behavior, authorization,
and the exact point at which deletion becomes irreversible.

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
