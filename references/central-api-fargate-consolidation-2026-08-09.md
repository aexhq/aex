---
title: Collapsing the central HTTP surface into one Fargate service
description: Why the central plane's 27 HTTP operations and the finance inbox move from four Lambda deployables behind API Gateway into one long-lived ECS Fargate service behind an ALB, what stays on Lambda, how credential verification moves in-process, and what the gateway gave that a load balancer does not.
keywords:
  - central plane
  - fargate
  - application load balancer
  - authentication
  - api gateway
audience: implementation agents and maintainers
status: accepted
date: 2026-08-09
related:
  - references/architecture.md
  - references/rewrite/central-identity.md
  - references/rewrite/central-finance.md
  - references/rewrite/delivery.md
---

# `central-api`: one long-lived central HTTP service

## The decision

The central plane serves its public HTTP surface from **one** long-lived ECS
Fargate service, `central-api`, in `eu-west-1`. The platform runs two long-lived
tasks in total: this one and `session-stream-api`. Large one-off and async work
stays on Lambda.

This is an owner decision, recorded rather than argued. What follows is what it
costs, what it changes and the one question it leaves open.

## What moves

Twenty-seven authored central operations, verified against
`api/generated/registries/routes.json`. Twenty-six are mounted today; one is
authored-but-unserved and stays that way.

| Today's deployable | Ops | Route group | Paths |
| --- | ---: | --- | --- |
| `central-control-api` | 16 | `api-keys`, `bootstrap`, `operations`, `organizations`, `workspaces` | `/api/api-keys*`, `/api/bootstrap`, `/api/operations*`, `/api/organizations*`, `/api/workspaces*` |
| `finance-api` | 8 | `billing` | `/api/billing/balance`, `/api/organizations/{id}/billing/*` |
| `central-identity-api` | 2 | `auth` | `/api/auth/device/authorizations`, `/api/auth/device/tokens` |
| _(deferred)_ | 1 | `identity` | `GET /api/account` — authored, never mounted, planned owner `central-identity-api` |

`finance-ingest` folds in as well. It serves no HTTP route today: it is a
direct-invoke Lambda whose `IngestRequest::ProviderEvent` arm is called
`RequestResponse` by `stripe-webhook-edge`, on the webhook's synchronous path.
Its `Healthz`/`Readyz` request arms exist only because a Lambda serving no HTTP
cannot expose `/internal/healthz` and `/internal/readyz` as routes; inside a
long-lived service those arms stop being a private protocol and become the two
real probes the service already has to serve.

The three route groups compose into one binary because they already share one
contract: every one of the 27 is `Plane::Central`, every one is dispatched by a
generated `dispatch_*` from `aex_wire::server`, and `aex_central_http::router`
already mounts all eight central groups by looping their generated route slices.
Nothing about the merge is new code paths; it is the removal of three process
boundaries between code that already agrees.

## What stays on Lambda, and why

Per the owner's placement policy, work that is queue- or schedule-driven,
externally triggered, or bursty and independent does not belong in a
request-path task:

| Stays | Why |
| --- | --- |
| `central-control-worker` | Queue- and schedule-driven, no public route. |
| `finance-settlement-worker` | Queue-driven settlement. |
| `finance-reconcile` | Scheduled. |
| `usage-receipt-dispatcher` | Queue-driven. |
| `provider-cost-reconciler` | Scheduled. |
| `stripe-command-edge`, `stripe-webhook-edge` | External, bursty, independent. They also hold the only Stripe secrets; keeping them separate keeps that blast radius where it is. |
| `central-schema-admin` | Already a one-shot ECS task (`rust-oci-task`, `desired_count = 0`). |
| `central-authz` | Only its **API Gateway REQUEST-authorizer half** becomes unnecessary. Its other half — the assertion issuer the five regional edges invoke over `lambda:InvokeFunction` — stays, is out of scope here, and must not be removed. |

## The hard consequence: authentication moves in-process

### What is true today

API Gateway cannot integrate an ECS service. `infra/modules/http-api-v2`
accepts only qualified Lambda alias ARNs (`integration_alias_arns`,
`authorizer_alias_arn`, both validated to reject `$LATEST`). Moving central
behind an ALB therefore removes the REQUEST authorizer, and with it the only
component that ever **verifies** a central credential.

The central services do not verify credentials. They trust a context map:

* `crates/aex-central-http/src/authorizer.rs` parses a **closed** twelve-key
  map (`CONTEXT_KEYS`), refuses an undeclared key, re-checks the context's shape
  per principal kind, and re-checks its window against a 30 s ceiling
  (`MAX_CONTEXT_LIFETIME_MS`).
* It never checks a credential, because it holds no secret with which to do so.

Behind API Gateway that is sound: the map can only have come from
`central-authz`, and the gateway will not forward a caller-supplied
`aex.*` authorizer context. Behind an ALB it is an **authentication bypass** —
the map becomes caller-controlled, and a request that simply asserts
`aex.principalKind=account` with any `aex.principalId` is admitted.

So verification must move into the process. Not be invented there: **relocated**.

### What is relocated, unchanged

From `services/central-authz/src/authorizer.rs`, in the same order:

1. `bearer` (~:171–204) — a single, unambiguous `Authorization: Bearer <value>`;
   refuses a comma, refuses two disagreeing values across single- and
   multi-value header maps, refuses internal whitespace.
2. `parse_supported` (~:206–217) — admits exactly `WorkspaceKey`,
   `AccountToken`, `DashboardSession` by prefix; `EmailChallenge` and
   `DeviceCode` are not central credentials and are refused.
3. `answer` (~:262–357) — resolves the credential against Aurora through
   `AuthorizationReader::{resolve_workspace_key, resolve_account_token_central,
   resolve_dashboard_session_central}`, then checks, per kind and in this order:
   identity echo (`state.key_id == credential.id`), the workspace/region pin,
   revocation, expiry, user activity, workspace status, organization status —
   and last, the MAC.
4. `credential_matches` (:359–373) — the peppered MAC verify,
   `verify(pepper, &credential.digest, &Verifier::from_bytes(stored))`, resolving
   the pepper by the version the **stored row** names.

Two invariants come with it and are not negotiable:

* **One undifferentiated refusal.** Every failing check above answers the same
  `401`. A caller must not be able to distinguish "no such key" from "wrong
  secret" from "revoked" from "organization suspended".
* **Unverifiable is not invalid.** An unreachable store, or a pepper version
  this process cannot resolve, is a *fault* (`503`), never a `401`. Answering
  "your credential is invalid" when the truth is "we could not check" is a lie
  the caller acts on by discarding a valid credential.

### Where it lands

A shared module in `crates/aex-central-http`, which already owns the central
context contract, already depends on `aex-control-app` and `aex-control-domain`,
and is already the only place a central principal is constructed. It gains a
dependency on `aex-identity-domain` (for `credential::{parse, verify, Verifier,
PepperVersion}`) and on `aex-identity-app` (for the `PepperKeystore` port).

The pepper is resolved through **`PepperKeystore::by_version`**, not through
`central-authz`'s flat `PepperRing`. Both hold the same primitive; the port is
chosen because `central-authz` is a `[[bin]]`-only crate nothing can depend on,
because `SecretsManagerPepperKeystore` already implements the port and
`central-control-api` already composes it, and because the port resolves *the
version the stored verifier names* rather than whatever the secret currently
holds — which is the difference between surviving a pepper rotation and reporting
every pre-rotation credential as invalid.

`central-authz` itself is untouched by this work.

### Where it runs

The relocated verification becomes stage 2 of a numbered, in-process admission
chain, on the model of `crates/aex-regional-http/src/edge.rs:251-371` — ten
stages, each numbered in source, each one early return in the wire contract's
order. The central chain already exists in
`aex_central_http::router::admit_edge` and already numbers its stages 1–9. It
changes in exactly one place:

```
1. Transport envelope — match_route against the one table.       (unchanged)
2. Authentication — WAS: read the gateway's context map.
                    NOW: parse the bearer, resolve it against Aurora,
                         verify the peppered MAC, and MINT the context.
                    Then verify it against its own window, as before.
3. Body bound.                                                   (unchanged)
4. Declared headers, strict in both directions.                  (unchanged)
5-6. Target resolution, decision, account-state gate.            (unchanged)
7. The generated handler's RequestContext.                       (unchanged)
8-9. Replay identity, strict in both directions.                 (unchanged)
```

The context hygiene in `aex-central-http` **stays**, and stays applied to the
locally minted context. Its closed key set and its per-kind shape check are no
longer defending against a compromised authorizer — they are now defending
against this crate's own minting code, which is a weaker threat but a real one,
and the checks cost nothing. The 30 s window ceiling stays for the same reason it
existed: a credential decision is a snapshot, and the gateway's 15 s
`authorizer_result_ttl_in_seconds` cache — deliberately half the ceiling — is
gone, so the window now bounds nothing but itself. It is retained as a shape
invariant, not as a cache bound.

The two device-flow routes keep their credential-free path: `admits_anonymous`
already reads the route table, and an absent `Authorization` header on those two
routes yields the one-millisecond anonymous context rather than a refusal. On
every other route, absent **or unverifiable** is `401`.

## What API Gateway gave that an ALB does not

Enumerated so that each is a decision rather than a discovery.

| Capability | Under `http-api-v2` today | Under an ALB |
| --- | --- | --- |
| **Request throttling** | `default_route_settings { throttling_burst_limit = 100, throttling_rate_limit = 50 }` (`main.tf:103-112`) | **Nothing.** See the open question below. |
| **Credential verification at the edge** | REQUEST authorizer, result cached 15 s, keyed on `$request.header.Authorization` | Moves in-process (above). No cache; every request resolves against Aurora. |
| **Per-route, per-method invoke scoping** | One `aws_lambda_permission.route` per operation, `source_arn` narrowed to that method and path | Collapses to **one target group**. The listener rule forwards a path prefix; nothing below it is separately authorised. Route-level authorisation is entirely the process's own `Action::central` + `requirement()` table now. |
| **No bypass endpoint** | `disable_execute_api_endpoint = true`; only the custom domain answers | **No analogue.** The ALB's own `*.elb.amazonaws.com` DNS name answers permanently. Closing it needs a host-header condition on every listener rule, which `alb-service-target` cannot express today (it emits `path_pattern` conditions only). |
| **Access logging** | CMK-encrypted CloudWatch log group, retention configured, fields include `authorizerError` and `integrationError` | Fixed-schema S3 access logs (`alb-public` mandates a bucket). No authorizer field — there is no authorizer. No integration-error field. Correlating a refusal to a cause moves entirely to the service's own telemetry. |
| **Route-set enumeration** | `routes` map is explicit; no `$default`, no `ANY`, no `*`; an unlisted operation is a 404 at the edge | The listener rule forwards `/api/*`. An unmounted path is a 404 from `axum`, one hop later, inside the task. |
| **TLS termination identity** | Regional custom domain, one stage mapping | ALB listener, `ELBSecurityPolicy-TLS13-1-2-2021-06` floor, `drop_invalid_header_fields = true`. Comparable; not identical in cipher/ALPN behaviour. |

Two further consequences that are not "loss" but are new:

* **A second ALB is a new standing cost.** The central plane has no load
  balancer today; `infra/examples/central-application` has no `vpc_id`, no
  public subnets, no ECS cluster and no `http-api-v2` instantiation at all. An
  ALB is billed hourly whether or not it is used. The alternative — attaching
  `central-api` as a second target group on the regional `eu-west-1` ALB, which
  already exists — is cheaper and is the literal reading of "two long-lived
  tasks", but it needs host-header conditions (different certificates, different
  public hostnames) that `alb-service-target` cannot emit, and it couples the
  `central-application` and `region-application` roots. Recorded as the first
  thing to reconsider if ALB cost matters; not taken here.

* **`finance-ingest`'s caller must change transport.** `stripe-webhook-edge`
  invokes it today with `FunctionName: AEX_FINANCE_INGEST_ARN`
  (`services/stripe-webhook-edge/src/handler.ts:143`). A Fargate service cannot
  be `lambda:Invoke`d. The inbox therefore needs a reachable path, and
  `alb-service-target` **refuses to forward `/internal*` from the public
  listener** and requires every pattern to start with `/api/` — by validation,
  with a test named `rejects_forwarding_an_internal_path`. So the ingest path is
  either (a) a public `/api/`-prefixed route with its own credential, which
  makes an internal protocol a public one; (b) a second, internal ALB, which
  `alb-public` cannot create (`internal = false` is hardcoded); or (c)
  `finance-ingest` stays on Lambda after all. This is the one part of the move
  that is not merely mechanical, and it is called out here rather than decided.

## Open question for the owner: throttling

**This is the item to decide before the move is applied.** It is stated, not
resolved, and no rate limiter has been invented in its place.

**The gap.** API Gateway supplies the only request throttle anywhere in the
central plane: `throttling_burst_limit = 100`, `throttling_rate_limit = 50`, at
`infra/modules/http-api-v2/main.tf:103-112`. There is **no WAF in either
repository** — the string appears nowhere in `infra/`. An Application Load
Balancer has no throttling of any kind. Once central moves behind one, the
central plane has no request-rate control at all.

**Why it matters most for two specific routes.** `POST
/api/auth/device/authorizations` and `POST /api/auth/device/tokens` are
unauthenticated **by design** — `admits_anonymous` returns true for both, and
every anonymous caller shares one replay principal
(`anonymous_principal()`), so two unrelated callers presenting the same
`Idempotency-Key` share one grant. `aex_central_http::router` carries a
`TODO(cross-stream)` at that exact function saying the accepted design puts a
per-IP API Gateway usage-plan throttle in front of these two routes *for exactly
this reason*, and that infrastructure owns the rule. Removing the gateway
removes the thing that TODO points at.

Every other central route now resolves its credential against **Aurora on every
request** — the 15 s authorizer cache is gone with the authorizer — so an
unthrottled credential-stuffing flood is also a direct load amplifier onto the
control database.

**Options, as I see them:**

| Option | What it buys | What it costs | Notes |
| --- | --- | --- | --- |
| **A. AWS WAF web ACL with a rate-based rule on the ALB** | The direct analogue: per-source-IP rate limiting at the edge, evaluated before the target sees the request. Can be scoped to the two device-flow paths with a URI-path scope-down statement, or applied plane-wide. | List price ≈ $5/mo per web ACL + $1/mo per rule + $0.60 per million requests. A new module (`infra/modules/waf-*`) — nothing to reuse. | Rate-based rules are per-IP over a fixed window; they do not express "50 rps overall". Closest to what is being lost, and the only option that also covers the ALB's own DNS name. |
| **B. Keep an API Gateway HTTP API in front, integrating the ALB via VPC Link** | Keeps the throttle, keeps `disable_execute_api_endpoint`, keeps per-route settings and the CMK-encrypted access log. | A VPC Link (≈ $0.01/hr) **plus** an internal ALB, which `alb-public` cannot create; plus the gateway request charge. Reintroduces exactly the hop this consolidation removes, and adds latency to every central request. | Honest fallback if the throttle is judged load-bearing. It does not restore the REQUEST authorizer — that removal is driven by the ALB, and in-process verification is required either way. |
| **C. CloudFront in front of the ALB** | TLS/edge termination, caching for nothing we serve. | Does **not** rate-limit on its own; still needs WAF. | Strictly worse than A unless CloudFront is wanted for other reasons. |
| **D. Accept the gap, plane-wide** | Zero cost, zero new infrastructure. | The two unauthenticated routes are open to unlimited issuance; authenticated routes amplify onto Aurora. | The device flow has intrinsic limits (one-time codes, a polling interval, short expiry) that bound the *value* of a flood, not its *volume*. |
| **E. In-process limiter in `central-api`** | Cheap, no infrastructure. | **Not recommended and not built.** It is per-task, so N tasks give N× the intended limit; it protects the process rather than the database; and it is precisely the "invent a rate limiter" this work was told not to do. | Listed only so the option is visibly considered and rejected. |

My reading, offered as input and not as a decision: **A, scoped down to the two
device-flow paths**, is the smallest thing that replaces what is being removed,
at roughly the cost of a rounding error against the plane's current spend. But
it needs a new Terraform module, and the instruction was to reuse
`alb-public`/`alb-service-target` and write no new ones — so this needs the
owner's word before anything is written.

## Blast radius of the unit change

Adding `central-api` and retiring four units touches more than
`release/units.toml`. Recorded so none of it is discovered late:

| File | What it needs |
| --- | --- |
| `release/units.toml` | The new `rust-oci-service` row; retirement of the replaced rows. `graph verify` requires `[unit.fargate]` with non-zero `cpu`/`memory_mb`/`stop_timeout_s`, a non-zero `port`, and both `health_path = "/internal/healthz"` and `ready_path = "/internal/readyz"` verbatim. |
| `Cargo.toml` | `services/central-api` and `tests/live/aex-live-central-api` as workspace members — `graph verify` fails `unit-package-unknown` otherwise. |
| `release/scenario-ownership.toml` | Some scenario must `observes` `artifact:central-api` — `missing-scenario-owner`. |
| `release/semantic-receipts.json` | A producer row per semantic receipt class in `required_receipts`. |
| `tools/aex-workspace-check/src/inventory.rs` | `SERVICES` and `LIVE_TARGETS`, both alphabetically sorted and test-checked. |
| `tools/aex-release-tool/src/manifest.rs` | `DEFAULT_ORDER` — every unit appears in exactly one stage. |
| `api/schemas/registries/routes-meta.yaml` | `servingArtifacts` and the pinned `expected.central` count. |
| `crates/aex-central-http/src/config.rs` | `CentralServiceId` — a `CentralApi` variant owning the merged group list; `every_actually_served_central_route_has_exactly_one_runtime_owner` asserts the runtime map equals the generated registry exactly. |
| `infra/examples/central-application/` | Has no `vpc_id`, no `public_subnet_ids`, no ECS cluster and no ALB today. The `alb-public` + `alb-service-target` + `ecs-service` triad in `infra/examples/region-application/main.tf` is the shape to copy. |
| `services/stripe-webhook-edge/src/handler.ts` | Only if `finance-ingest` actually folds in — see the transport question above. |

## The service shape

`services/central-api`, following `services/session-stream-api/src/main.rs` as
the long-lived Fargate root:

* telemetry installed **first**, via `LongLivedTelemetry::install` — not the
  Lambda profile, which flushes on an invocation boundary this process does not
  have;
* configuration from the environment with an explicit `keys::ALL` required list,
  every value refused rather than defaulted, and the list printed on refusal;
* every start-up probe — Aurora, the pepper keystore, the cursor secret, the
  regional endpoint map — run **before** the listener binds; a process that
  cannot verify a credential must not serve;
* one readiness flag shared by `/internal/healthz` and `/internal/readyz`,
  carrying a drain signal;
* its own `TcpListener`, bound after admission of configuration and key
  material;
* on `SIGTERM`: raise the drain flag **first**, so `/internal/readyz` answers
  `503` and the ALB deregisters the target while in-flight requests finish;
  *then* ask the listener to stop; then bound the wait by a drain deadline set
  **below** the ECS `stopTimeout`, so the process exits on its own terms rather
  than being killed mid-transaction.

Sized from `session-stream-api` as the peer of the same class: `cpu = 1024`,
`memory_mb = 2048`, `stop_timeout_s = 30`, `port = 8080`. The reasoning that
justified those numbers there — per-request work rather than per-task work, the
transaction compiler and the JSON codec being what the task must hold — is the
same reasoning here. `desired_count` is a per-plane root concern, not a registry
one.

## What composing the binary actually requires

Two of the three deployables cannot be composed into another binary as they
stand, and this is the largest single item of remaining work:

* `services/central-control-api` has **no library target** — `Cargo.toml`
  declares only `[[bin]]`, and `ControlService` lives in a `mod api` private to
  that binary. `services/central-identity-api` is the same shape (`main.rs` plus
  private `api`, `startup`, `targets` modules).
* `services/finance-api` **does** have a library target, but it mounts its
  billing group through its own edge (`finance_api::edge::CentralEdge`,
  `UnresolvedPrincipalEdge`) rather than `aex_central_http::router::EdgeStack`,
  with its own `policy::replay_key` and `policy::scope` steps. Both produce the
  same `aex_wire::server::RequestContext`, and `aex-central-http` already has a
  `mount_billing_api`, so the mount is compatible; what is not yet settled is
  whether billing's target resolution and organization gating come out the same
  through `ControlStoreTargets` as they do through finance's own edge.

So the merge is: give the two control/identity deployables library targets (or
lift their services into crates), reconcile finance's two extra policy steps
against the shared stack's route-table-driven equivalents, and only then write
the composition root. None of that is speculative work — it is the shape the
merge takes — but it is not a mechanical edit, and doing it badly on the path
that now carries authentication is worse than doing it in a second pass.

## Status of this work

Landed:

* this design record;
* the authentication relocation — `crates/aex-central-http/src/admission.rs`,
  wired into the numbered admission chain, with the ambient context ignored
  outright once a composition verifies credentials itself.

Not landed, in the order it should be taken up: the library targets above, the
`services/central-api` composition root, the `release/units.toml` row (which
cannot land first — `graph verify` fails `unit-package-unknown` for a unit whose
package is not a workspace member), the registry rows in the blast-radius table,
and the Terraform. The throttling question above gates the last of those.
