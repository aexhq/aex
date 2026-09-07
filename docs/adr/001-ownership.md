# ADR-001: Keep Brain, public hosting behavior, and private operations separate

Status: Accepted. Date: 2026-09-07.

## Context

The owner requests a complete Aex rewrite, a future-public Aex repository and a private
Platform repository. Consistency, low coupling, high cohesion, separation of concerns and
Occam's razor govern code. Performance, scalability and maintainability govern operations.
The retired product is reference material, not a compatibility or deployment requirement.

## Decision

Brain owns session execution, canonical history, projections, model protocols and Environment
mechanisms. Aex consumes these mechanisms and owns account identity, resource authorization,
hosted admission and product operations. Platform owns our deployment, integration secrets,
customer operations, commercial settings and release evidence.

Use a modular service, not independently deployed services for accounts, sessions, keys and
quotas. These functions currently participate in one admission boundary and share transactional
product state. Brain remains an independently versioned process under ADR-002.

Intended structure, created only as implementation needs it:

```text
aex/
  crates/aex-server/
    src/{main,config,http,identity,sessions,hosts,brain,store}.rs
    migrations/
  tests/journeys/
  examples/
  docs/adr/
  Dockerfile
  Cargo.toml
  Cargo.lock
  .github/workflows/
```

`http` extracts and validates external requests; `identity` resolves credentials and ownership;
`sessions` and `hosts` coordinate their product operations; `brain` owns the Brain transport;
`store` owns product queries and transactions. Configuration is parsed once at startup.
Use explicit principal arguments; no implicit tenant globals. Modules may become directories
when they contain enough related code. Do not create empty package trees in advance.

External entry points call the owning operation. Neither the Brain client nor storage queries
decide customer policy. Keep SQL in its owning storage module and expose concrete operations,
not a generic repository framework. Add traits only for real substitution or a needed test seam.

Dependencies flow from Platform's deployment to public artifacts, and from Aex to Brain.
Public builds and tests require neither private source nor live commercial credentials.
Reusable security/admission behavior belongs in Aex; our chosen limits and commercial values
belong in Platform. Private business logic, if later necessary, gets a narrow integration
only when that logic exists. There is no speculative `BusinessPolicy` plugin framework.

Public docs describe supported behavior and guarantees. Public API semantics cannot be hidden
as “business details.” Private account identifiers, topology settings, margins, customer data
and operational history do not enter public source or build artifacts.

## Alternatives and consequences

- A public SDK with the entire service private would weaken the proposed public hosting
  product. Keep it as an explicit owner choice, not an accidental result of the split.
- Microservices would add network and release coordination before independent load or team
  boundaries exist. Separate cohesive modules now; extract only when measured need demands it.
- Rebuilding Brain inside Aex would create two owners for the same semantics and make each
  Brain upgrade an integration rewrite.
- A new SDK/package for every conceptual boundary would duplicate existing Brain contracts.
  Initially the Brain SDK is the customer session SDK.

This follows the owner's code guidance and the reuse-before-construction approach described
by [Ponytail](https://github.com/DietrichGebert/ponytail). Small code remains responsible for
validation and failures at real trust boundaries.

## Acceptance and reconsideration

Accept when the public hosting-service boundary is confirmed. Verify a clean public checkout
can build and run its local journey without Platform. Reconsider module extraction when a
component needs a genuinely separate deployment, consumer or scaling profile.
