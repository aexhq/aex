---
title: Identifying a sandbox to the platform for platform-paid tool calls
description: How a tool running inside a customer-controlled microVM can cause the platform to spend its own credential on that customer's behalf without holding anything it could forge, move, or leak — why the guest should carry no credential at all, why a workspace API key in the guest is the wrong answer, and where the spend bound has to live.
keywords:
  - sandbox identity
  - hands
  - microvm
  - platform credential
  - tool execution
  - impersonation
  - rate limiting
audience: implementation agents and maintainers
status: proposal
date: 2026-08-09
related:
  - references/rewrite/hands.md
  - references/rewrite/tools-mcp.md
  - references/rewrite/usage.md
  - references/architecture.md
  - crates/aex-regional-http/src/edge.rs
  - crates/aex-regional-http/src/credential.rs
  - crates/aex-hands-control-aws/src/provider.rs
  - crates/aex-brain-app/src/ports/proof.rs
---

# Identifying a sandbox to the platform

A `web_search` that the platform pays for needs an answer to one question: when
something inside a customer's microVM asks for work to be done with the
platform's money, who is asking, and what stops them from saying they are
somebody else.

This document proposes an answer. It is a proposal, not a decision. Everything
marked **verified** was read in this repository at commit `f0661f41`; everything
marked **inferred** is reasoning from what was read and is flagged where it
matters.

The short version: **the guest should hold nothing, and the identity of a
platform-paid call should be positional rather than asserted.** The platform
already knows which tenant a microVM belongs to, because the platform launched
it, addressed it, and holds the only connection to it. Nothing the guest says
needs to be believed, because the guest does not need to say anything.

## 1. What is true today

### 1.1 The guest is credential-free, and that is load-bearing

**Verified.** There is no secret, token, nonce, launch ticket, IAM role, or
instance credential inside a Hands microVM. The absence is deliberate, is stated
in code, and is asserted structurally rather than by convention.

- The launch request has **no** `execution_role_arn` field at all.
  `crates/aex-hands-control-aws/src/provider.rs:10-14` says why: *"That is the
  control: a role cannot be threaded through, defaulted in a config, or spread
  from a caller's struct, because there is nowhere to put it."*
- The only per-launch data is the run-hook payload, whose key set is closed,
  sorted and `deny_unknown_fields`:
  `["bounds","capabilities","generation","imageDigest","protocolVersion","root","size","v"]`
  (`crates/aex-hands-agent/src/boot.rs:97-106`, mirrored at
  `provider.rs:159-168`). A payload carrying `apiKey`, `launchTicket` or
  `credentials` is refused by the decoder, and there are tests that plant exactly
  those keys.
- The three environment variables in the guest are `AEX_HANDS_LISTEN_ADDR`,
  `AEX_HANDS_JOURNAL_ROOT`, `AEX_HANDS_GUEST_ROOT` (`boot.rs:23-32`), identical in
  every VM, none secret.
- The guest binary links no HTTP client, no TLS stack and no cloud SDK.
  `the_binary_carries_no_tls_stack_it_could_reach_a_cloud_endpoint_with`
  (`runtimes/hands-agent/src/main.rs:430-443`) fails if `reqwest`, `rustls`,
  `hyper-rustls` or `native-tls` appear anywhere in the `cargo metadata`
  dependency closure, with the failure message *"the guest links `{client}`, which
  is how a credential-free boundary stops being one"*;
  `crates/aex-hands-agent/tests/no_cloud_authority.rs` scans the same closure for
  `aws-*`, `rusoto`, `azure_`, `google-cloud` and the AEX cloud crates, with a
  self-check proving the matcher works. Note what that first test's own comment
  admits: workspace materialize and persist need presigned HTTPS and are therefore
  *refused by this guest and recorded as a gap rather than quietly linked in*.
- The customer is root. `OS_CAPABILITIES = "ALL"`
  (`runtimes/hands-image/src/image.rs:48`), the in-guest firewall was deleted
  rather than kept, and the code says why: *"root can flush any nft table, and
  the IMDS rule protected an execution role this target never attaches"*
  (`image.rs:122-125`).

The threat model is already written down, at `provider.rs:127-132`:

> A credential-free guest can call no authority, so there is nothing for a ticket
> to protect. Forging this payload requires launching a MicroVM in AEX's own
> account; rewriting it from inside the guest breaks only that customer's own
> session.

**This is the property to preserve.** Every design below is judged first on
whether it keeps it.

### 1.2 The channel is inbound only

**Verified.** The platform dials the guest. The guest never dials anything.

`brain-mux` resolves the microVM's endpoint from `RunMicrovm`/`GetMicrovm`,
validates the URL hard before attaching anything (https, no userinfo, port 443 or
absent, root path, no query, no fragment —
`crates/aex-brain-hands/src/guest.rs:248-278`), then sends a binary frame with two
headers: `X-aws-proxy-auth` and `X-aws-proxy-port: 8080`
(`guest.rs:25-26, 221-227`).

That auth token is minted by **AWS**, not by AEX
(`CreateMicrovmAuthToken`, `crates/aex-hands-control-aws/src/aws.rs:162-194`). It
is scoped to exactly one microVM and exactly one port — the adapter refuses any
other port list — lives 1800 seconds, is held only in a bounded in-memory LRU,
and has no `Display`, no `Serialize` and a redacting `Debug`
(`provider.rs:270-325`). The AWS proxy terminates TLS and re-originates inside the
VM, so the guest speaks plain HTTP and never observes the header
(`runtimes/hands-agent/src/serve.rs:1-9`).

The protocol is five verbs on one port: `start`, `status`, `cancel`, `result`,
`attach` (`crates/aex-hands-agent/src/wire.rs:79-92`). `attach` currently answers
`501`. Every frame carries a generation binding and a monotone fence, both checked
before payload decode (`crates/aex-hands-protocol/src/rpc.rs:488-524`), plus an
`agent_build[8]` stamp — the first eight bytes of the running binary's own blake3
— so Brain detects a swapped agent without trusting the guest's account of itself
(`main.rs:136-150`, consumed at `crates/aex-brain-hands/src/backend.rs:688-716`).

**There is no vsock, no virtio channel, no unix socket to the host, no shared
file, and no stdio pipe.** Repository-wide search for `vsock`, `AF_VSOCK`,
`virtio`, `UnixStream`, `UnixListener` finds nothing in any Hands crate. One TCP
port is the entire interface.

### 1.3 Network position: `none` or raw internet, with no middle

**Verified.** Two modes, closed at the schema, wire and domain layers:

```rust
pub enum NetworkPolicy {
    /// The managed `INTERNET_EGRESS` connector, and nothing else.
    PublicInternet,
    /// No egress connector at all.
    None,
}
```
`crates/aex-runtime-control/src/generation.rs:119-127`; wire twin at
`crates/aex-wire/src/generated/models.rs:2941-2962`; schema source at
`api/schemas/regional/sessions.yaml:30-35`.

`PublicInternet` attaches the AWS-managed `INTERNET_EGRESS` connector and nothing
else. There is no allowlist, no proxy, no DNS control and no filtering of any
kind. (Do not confuse this with
`crates/aex-brain-managed-web/src/egress.rs`, which is the *platform's own*
SSRF-screened outbound policy running inside `brain-mux`. A guest on
`public_internet` bypasses it by opening a socket.)

Three consequences that matter here:

1. **The platform's regional API is reachable from a `public_internet` guest.**
   `session-stream-api` sits behind a public ALB admitting `0.0.0.0/0:443`
   (`infra/modules/alb-public/main.tf:24-42`, wired at
   `infra/examples/region-application/main.tf:69-78`). The declared boundary
   control B2 is *"no AEX VPC egress or private route"*
   (`references/rewrite/hands.md:309-310`) — a public hostname is not a private
   route, so reaching it does not violate B2 as written. Authorization is the
   control, not the network.
2. **`none` is the fixture posture everywhere, but nothing enforces it.** The
   request field is optional with no documented default
   (`sessions.yaml:122`, contrast `size` at line 79 which does document one), and
   no code resolves a requested `NetworkMode` into a `NetworkPolicy` — every
   `NetworkPolicy` value constructed in the workspace is `::None` and all are
   test fixtures. A default is a convention here, not a type. *(Inferred: the
   intent is `none`, from the fixtures and from `references/architecture.md:113-119`.)*
3. **Any design that requires the guest to reach the network is unavailable
   under `none`.** This turns out to be decisive.

### 1.4 `web_search` today is BYOK, runs in Brain, and is not advertised

**Verified.** `crates/aex-brain-managed-web/src/search.rs:1` — *"Closed BYOK
web-search authority and provider normalizers."*

- The catalog entry (`crates/aex-brain-tool-catalog/src/catalog.rs:465-479`) has
  `ToolBoundary::ManagedWeb`, `EgressClass::ManagedInternet`, 15 000 ms timeout,
  concurrency weight 4, and usage dimensions compute+memory+data_transfer.
- It is the **only** tool in the 33-entry catalog with
  `CredentialClass::WorkspaceSecret`, name `aex_web_search`
  (`catalog.rs:111-120`). The customer supplies their own Brave or Serper key.
- The credential is decrypted per dispatch through the session custody path —
  head read, binding read, encryption-context digest compare, durable authorize
  write, then KMS reveal, in that exact order, asserted by a test
  (`crates/aex-brain-managed-web/src/credential.rs:157-227, 531-534`).
- **It is not advertised in production at all.**
  `runtimes/brain-mux/src/wake.rs:791` hardcodes
  `let secrets = ResolvedSecretNames::default();` — always empty — so readiness
  drops the entry and `route()` answers `NotAdmitted`. A test pins this
  (`wake.rs:1417`). The live advertised surface is
  `{todo_read, todo_write, web_fetch}`.

There is also design-intent language already in the tree that the code has not
implemented. `references/rewrite/observations.md:904-910`:

> the platform's own credentials never enter the customer sandbox. When the model
> calls a tool such as `web_search`, the platform makes the outbound request with
> its own key and hands the results back; the sandbox holds results, never the key.

The *location* half is true today. The *"with its own key"* half is not: there is
no platform-credential path anywhere in the repository. **Verified by absence** —
the only Secrets Manager consumer is the identity pepper in
`crates/aex-central-aws/src/pepper.rs:215`, and `brain-mux` holds no
`secretsmanager` grant.

### 1.5 Tools are not executables today

**Verified.** The image contract requires exactly one executable,
`/opt/aex/hands-agent` (`crates/aex-hands-agent/src/image_contract.rs:36-73`), and
explicitly forbids `/opt/aex/credentials`, `/root/.aws`, `/opt/aex/runner-bundle`
(lines 75-84). The guest protocol is a closed typed enum of operations
(`Exec`, `ReadFile`, `WriteFile`, `Search`, …), not an exec-a-tool-binary
dispatch. The one extension point, `RegisteredTool`, is dead:
`HandsToolExecutor::supports()` returns `false` for everything
(`crates/aex-brain-hands/src/executor.rs:77-83`).

So "every tool is an executable in the image" is a new architecture, not a
refinement of the current one. That is fine, but it means none of the existing
guarantees about tools carry over automatically.

### 1.6 The credential model to reuse, not reinvent

**Verified.** `crates/aex-regional-http/src/edge.rs` runs ten in-process stages.
The shape worth internalizing:

- Stage 3 is **one reconciled projection read per request, never cached**. There
  is no window: a revoked key or a paused account takes effect on the next
  request. This is what commit `04ad0582` bought by deleting the assertion.
- Stage 4 is a peppered MAC — `HMAC-SHA256(pepper_v, SHA-256(token))` — compared
  in constant time against a verifier replicated onto the regional row. The
  reasoning at `credential.rs:15-19` matters for what follows: *"Holding the
  pepper and the verifier lets a holder **check** a presented key; it does not let
  them mint one."*
- **The platform never retains a customer's API key plaintext.** Only the
  verifier is stored. This single fact disposes of the sketched design in §2.
- `PresentedCredential::new` accepts a workspace API key **and nothing else**
  (`credential.rs:62-76`). One credential kind at a regional edge is a deliberate
  invariant, not an accident.
- `AudienceSet` is a `u8` bitset over five audiences
  (`crates/aex-internal-contracts/src/assertion.rs:120-128`), so three bits are
  free. The empty set is representable and admits nothing — fail-closed by
  construction.

Why `04ad0582` removed the signed assertion is the design constraint to carry
forward: it was *"a synchronous Lambda invoke, cached 30 s per credential, and a
single point of failure for every regional service in every region at once."*
**Do not reintroduce a per-call mint from a central authority.**

### 1.7 There is one identity value in the codebase that is already unforgeable

**Verified, and this is the pivot of the whole proposal.**
`crates/aex-brain-app/src/ports/proof.rs:134-167`:

```rust
pub struct DispatchTicket {
    effect: EffectId,
    attempt: u16,
    fence: Fence,
    key: AgentKey,
    at: Timestamp,
    workspace: WorkspaceId,
    organization: OrganizationId,
}
```

with the comment on `mint`:

> Requiring the guard is the point: a ticket cannot exist without proof of
> ownership, so a losing owner cannot manufacture permission to dispatch.

A `DispatchTicket` can only be minted from a `FenceGuard` — proof that this Brain
instance claimed this agent and has not been fenced out. It carries the workspace
and the organization. It is already what resolves the tenant's BYOK credential:
`WebSearchCredentialSource::resolve(&DispatchTicket)`
(`crates/aex-brain-managed-web/src/executor.rs:35-41`), whose doc requires
implementations to *"not return a credential from another tenant"* and notes there
is *"no process-global fallback"*.

The platform already has a tamper-proof, tenant-bound, per-effect identity for
tool dispatch. It is minted inside Brain from proof of ownership, and **no guest
input contributes to it.** The question is not how to build such a thing. It is
how to avoid throwing it away by asking the guest to assert an identity instead.

### 1.8 Metering and limits: what actually bounds spend

**Verified, and the answer is close to nothing.**

Four priced meters, all AWS-resource-shaped
(`crates/aex-usage-domain/src/meter.rs:167-176`):
`compute.millicpu_ms.v1`, `memory.byte_ms.v1`, `storage.byte_min.v1`,
`data_transfer.egress_byte.v1`. The module doc says *"Four meters are priced and
nothing else ever is."*

A per-call counter does exist, and it is deliberately unpriceable. `meter.rs:258-272`:

```rust
/// Zero-dollar BYOK observability counters.
///
/// Structurally disjoint from [`Meter`]: there is no conversion, no shared
/// trait and no rate path, so no rate can ever be looked up for one.
pub enum ObservabilityMeter { ModelTokens, ProviderCalls, ToolInvocations }
```

`ToolInvocations` is exactly the shape a platform-paid tool call needs, and the
type system is built to stop it carrying money. The rate card is
`BTreeMap<Meter, Rate>`, so an observability id cannot index it, and the outbox
refuses to publish an observability fact at all
(`OutboxError::Unrepresentable`, `crates/aex-usage-app/src/outbox.rs:418`).

That disjointness is correct for BYOK, where a tool call costs the platform
nothing beyond the bytes. It is exactly wrong for a call the platform buys.
Pricing one therefore means either a fifth `Meter` or breaking a separation the
code was written to guarantee. Neither is a small change, and it is why D-1 is a
decision rather than a detail.

Two further facts about the metering path, both **verified**:

- **`web_search` declares compute+memory+transfer** (`CMT`), which prices the
  bytes we move, not the vendor call we purchased.
- **Billing defaults to shadow.** `BillingMode::Shadow` is *"the default until
  `METER-03` passes"* and finance posts zero
  (`crates/aex-usage-app/src/worker.rs:44-52`), with a start-up gate that refuses
  to boot if the declared mode and the queue's `.fifo` suffix disagree. Whatever a
  new meter records, it records nothing chargeable until that flips.

`EffectiveLimits` is four size bounds and nothing else
(`crates/aex-regional-http/src/context.rs:84-93`):
`json_body_bytes`, `otlp_body_bytes`, `query_page_items`, `query_page_bytes`.
There is no request-rate field.

**There is no request-rate limiting in the platform.** The only rate control
anywhere is a WAF rate-based rule pointed at the two *unauthenticated*
device-flow routes on the central plane
(`infra/modules/waf-rate-limit/main.tf:1-26`), and its own module doc says API
Gateway *"carried the only request throttle anywhere in the central plane"* before
the ALB migration removed it. Searches for `TokenBucket`, `RateLimiter`,
`governor`, `usage_plan` over `crates/`, `services/`, `workers/` and `infra/`
return only inbound 429-handling from upstream model providers, which is the
opposite direction.

The sharpest illustration: **`telemetry.ingest_rate` is a declared limit with
published defaults that nothing enforces.** It is in the registry
(`api/schemas/registries/limits.yaml:10`) with dimensions
`sustained_bytes_per_second`, `burst_bytes`, `sustained_requests_per_second`,
`burst_requests`, and defaults of 10 MiB/s and 200 rps
(`api/schemas/registries/limit-defaults.v1.json:31-39`). A repository-wide grep
for `ingest_rate` finds it in exactly three places: the generated enum, the
registry YAML, and the defaults JSON. **No enforcement point exists.** Declaring a
limit in this repository does not create one.

The only mechanism that halts paid work is the account-state gate, and it is worth
tracing precisely, because the answer is not "it does not exist" — it is "it
exists, it is fail-closed, and nothing ever pulls the lever."

The chain is complete and wired end to end:

1. `finance.billing_account.state` — a stored column, not a computed one.
2. `finance.account_state_v1`, a view mapping `'active'` to `active` and
   **everything else, including a state added later, to
   `paused_top_up_required`** (`migrations/central/20260801000700_finance_account_state.sql:31-38`).
   The `ELSE` is deliberate.
3. The control plane reads it with a `LEFT JOIN` and `COALESCE`s a missing row to
   `unavailable`, which is a `503` (`crates/aex-control-aurora/src/sql.rs:22-36`).
   **Absence is never `active`.**
4. It projects onto the regional key authorization row, where edge stage 8 refuses
   a non-exempt route (`edge.rs:331`) and `revalidate` terminates an already-open
   lease (`edge.rs:254`). Neither read is cached.

That is a good mechanism. The problem is the input. **No production code path ever
writes `finance.billing_account.state`.** `finance.ensure_account` inserts
`state = 'active'` in the same transaction as the organization
(`20260801000700_finance_account_state.sql:51-56`), and the only production
`UPDATE finance.billing_account` in the tree sets the automatic top-up policy —
`auto_topup_enabled`, threshold, amount — and does not name `state`
(`services/finance-api/src/aurora.rs:72-78`). The other two occurrences are in a
migration test.

The prepaid credit model exists too, and is also unreached. There is a chart of
accounts with `CustomerAvailable` and `CustomerReserved`
(`crates/aex-finance-domain/src/account.rs:31-35`) and a pure admission function
that blocks on exhaustion:

```rust
} else if available.get() <= reserved.get() {
    AdmissionDecision::Block(BlockReason::InsufficientCredit)
```
`crates/aex-finance-domain/src/billing_account.rs:68-81`. Its only caller anywhere
is its own crate's test module. *(Note a name collision when searching:
`aex_brain_app::activation::AdmissionDecision` is a different, unrelated type
about memory permits.)*

`BillingAccount::spend_cap` exists, but read what it guards: it appears once, in
`auto_topup_decision`, and blocks **automatic recharge**
(`billing_account.rs:161-165`). It bounds how fast the balance is refilled, not how
fast it is spent. Since nothing consults the balance before admitting work, a
reached spend cap stops the top-up and changes nothing else.

So: a customer whose prepaid credit reaches zero keeps working. **Nothing sets the
pause automatically from spend, and nothing reads the balance on any admission
path.**

### 1.9 There *is* a working per-session budget, and it has an empty money slot

**Verified, and this is the most useful thing in this section**, because it means
§7 extends a mechanism rather than inventing one.

`crates/aex-brain-domain/src/budget.rs:15-33` defines a seven-dimension budget
charged up an agent tree, with exhaustion in any dimension producing
`FinishReason::Budget` (`planner.rs:187-192`). The launch grant
(`budget.rs:382-392`):

```rust
grant.set(Dimension::TotalChildrenCreated, 16_384);
grant.set(Dimension::ProviderCalls,        u64::MAX);
grant.set(Dimension::HandsCalls,           4_096);
grant.set(Dimension::CostMicroUsd,         cost_micro_usd);
grant.set(Dimension::ActiveChildren,       256);
grant.set(Dimension::QueuedChildren,       4_096);
grant.set(Dimension::RetainedResultBytes,  256 * 1_024 * 1_024);
```

Five of these genuinely halt an agent tree. Two do not:

- **`CostMicroUsd` is never consumed in production.** The dimension exists, the
  arithmetic exists, the halt exists — and its own doc comment says why it is
  unused: *"`$` — cost in micro-USD. **Under `BYOK` a model call does not touch
  this.**"* Every `consume(Dimension::CostMicroUsd, …)` in the tree is a test.
- **`ProviderCalls` is granted `u64::MAX`**, so it is off by construction.

That is a money dimension, wired end to end, already fenced to a session, already
producing a clean terminal reason, sitting empty because nothing has ever cost the
platform money before. **A platform-paid tool call is the first thing that should
touch it.**

Two smaller bounds worth knowing: `Dimension::HandsCalls` caps a session at 4096
guest calls, and `DEFAULT_MAX_TOOL_CALLS: u16 = 128` caps one turn
(`crates/aex-brain-provider-gateway/src/budget.rs:33`, enforced by
`BudgetLedger::open_tool_call`). Both are structural per-session or per-turn
bounds, not spend caps, and neither is per-organization.

One amplification number worth holding onto: `ActiveChildren` is 256, matching
`session.materialized_agents` in the limit registry, and the per-generation
operation ceiling is up to 32 concurrent
(`crates/aex-runtime-control/src/shape.rs:90-93`). Any budget expressed per agent
is a budget multiplied by 256; any budget expressed per session is multiplied by
however many sessions the customer opens.

## 2. The sketched design, and the attack that breaks it

> invoke the tool on a protected path that holds the user apikey and invoke a
> lambda

There are two ideas here. One of them is right and one of them breaks. Taking the
second first.

### 2.1 It cannot be built without a strict regression in credential custody

**This is the blocking objection, and it is structural rather than a matter of
taste.** The platform does not have the customer's API key. It stores
`HMAC-SHA256(pepper_v, SHA-256(token))` and nothing else
(`crates/aex-identity-domain/src/credential.rs:31-36`); the plaintext exists only
in the customer's hands and transiently inside a `Zeroizing` wrapper during a
request.

To place the customer's key in the guest at launch, the platform would have to
begin **retaining customer API key plaintext** in a store it can read at
generation-launch time. That creates the single highest-value target in the
system, in a plane that currently has none, in order to hand the secret to the
one party the design treats as hostile. Nothing else in the proposal is worth
that.

### 2.2 Even granting a key in the guest, the blast radius is the whole workspace

A workspace API key is not a tool credential. It carries the workspace's entire
effective scope set, computed centrally and read from the row on every request
(`edge.rs:117, 322-326`) — `sessions:write`, `secrets:read`, `files:write`,
`telemetry:write`, and the rest of the generated `ScopeId` vocabulary. It is
region-pinned and nothing else: not session-pinned, not tool-pinned, not
time-bounded.

The concrete attack:

1. The agent in the sandbox fetches a web page, clones a repository, or installs a
   package — all first-class supported activity (`python3`, `nodejs`, `git`, `gcc`
   ship in the image as customer tooling, `image.rs:71-103`).
2. That content contains an injection, or the dependency is malicious. It does not
   need a sandbox escape; it is already running as root inside the guest, which is
   the design.
3. It reads the key from `/proc/<pid>/environ`, the file it was written to, or the
   process's memory.
4. It now holds a credential that authenticates to the public regional API from
   anywhere on the internet, with `secrets:read` — which is the path to the
   customer's own BYOK provider credentials — and with `sessions:write`, and it
   keeps it after the microVM is gone.

The victim here is the customer whose sandbox it is, not another tenant. That
makes it less interesting for cross-tenant impersonation and *more* interesting as
a product defect: the platform would be the reason a customer's own agent
compromised their own workspace. The response to a leak is also all-or-nothing —
revoke the key their CI also uses.

### 2.3 "Protected path" is obfuscation

Stated for completeness because the brief asks for it. The customer enumerates
the filesystem, straces the binary, reads its environment and dumps its memory.
A design whose confidentiality rests on the customer not looking is not a design.
This is the least of the three objections, not the most.

### 2.4 It also does not work in the posture the platform intends to ship

Under `NetworkPolicy::None` the guest has no egress connector, so a tool that
reaches a public platform endpoint simply fails. `none` is the fixture value
everywhere and the value in the public architecture example. The safest posture
would be the one where the tool does not work.

### 2.5 The part that is right

*"the lambda holds the platform credentials that does the actual execution so no
credentials can ever leak"* is exactly correct, and it is the shape of the
recommendation below. The isolation of the paying process from the sandbox is the
whole point.

**The correction is only about who calls it.** The caller should be Brain, which
already holds an authenticated connection to that specific microVM and already
knows which tenant it belongs to — not the guest, which knows nothing the platform
should believe.

## 3. Proposal: the guest holds nothing, and Brain brokers the paid leg

### 3.1 The mechanism

A platform-paid tool call is executed by `brain-mux` under a `DispatchTicket`, and
the guest's role is to *ask*, never to *authenticate*.

Under today's routing this is already what happens: `web_search` has
`ToolBoundary::ManagedWeb`, the router dispatches it to the `ManagedWeb` executor
slot, and the microVM is not involved at all. That path needs no new identity
mechanism whatsoever. If the owner's larger proposal is adopted and
`web_search` becomes an executable inside the image, the following keeps the same
property.

**The broker channel.**

1. The model emits `web_search(query)`. Brain folds it, routes it, writes the
   durable prepare effect, takes a concurrency lane permit and mints the
   `DispatchTicket` from the `FenceGuard`
   (`crates/aex-brain-app/src/activation/run.rs:1570-1660`). All of this exists.
2. Brain dispatches an operation into the guest over the existing inbound channel,
   as it does for any Hands tool. Inside the guest the operation runs
   `/opt/aex/tools/web_search`.
3. That executable does **not** open a socket to the internet. It connects to a
   guest-local unix socket, `/run/aex/broker.sock`, served by `hands-agent`, and
   writes `{tool, arguments}`. It blocks.
4. `hands-agent` places the request in a bounded FIFO of broker slots on that
   operation. A slot carries `{slot: u32, tool: ToolName, arguments_jcs: Vec<u8>}`
   and **no principal field of any kind**.
5. Brain, which holds the operation open, observes the pending slot — on the
   existing `status` response, or streamed once `attach` ships (it currently
   answers `501`, `serve.rs:246-258`).
6. Brain performs the outbound call **itself**, in `brain-mux`, with the platform
   credential, through the existing `aex-brain-managed-web` egress policy: HTTPS
   only, port 443, no userinfo, full IPv4/IPv6 deny table, DNS screening with a
   pinned address set, manual redirects capped at five, each hop re-screened.
7. Brain answers with a new `broker_reply { operation, fence, slot, status,
   payload }` verb on the same connection. The payload is validated against the
   tool's declared result schema (`catalog.rs:1189-1197` for `web_search`) with
   `deny_unknown_fields`.
8. `hands-agent` unblocks the executable, which writes bytes to stdout.

The frame codec, the generation binding check, the fence check and the
`agent_build` stamp all apply unchanged, because a broker reply rides the same
preamble as every other verb.

### 3.2 What the guest holds, and who issues it

**Nothing, and nobody.** There is no token to issue, no lifetime to choose, no
revocation list to maintain, and no key rotation to schedule.

The guest holds what it holds today: a generation id, an image digest, bounds, a
root path, a compute size, a protocol version. All public, all in the closed
run-hook key set, none of them authenticating anything.

### 3.3 How it is bound to one session and one workspace

The binding is established entirely on the platform side, from four facts the
guest cannot influence:

| Fact | Where it comes from |
| --- | --- |
| This connection reaches exactly this microVM | The endpoint came from `GetMicrovm` in AEX's own account; the `X-aws-proxy-auth` token is AWS-minted, scoped to one microVM and to port 8080 only, refused by `AuthenticatedGuestEndpoint::new` if the port scope is anything else (`guest.rs:55-61`) |
| This microVM is this generation | `GenerationId` on the generation head, and the frame's generation binding is checked before payload decode on both sides |
| This generation is this session, workspace and organization | `HandsGeneration { generation, session, workspace, organization, … }` (`crates/aex-runtime-control/src/generation.rs:132-153`) |
| This Brain instance is entitled to act for this agent | `DispatchTicket` cannot be minted without a `FenceGuard` (`proof.rs:150-167`) |

The credential resolution and the spend attribution both key off the
`DispatchTicket`, which is the same value the tenant-BYOK path already uses.

### 3.4 How the platform verifies it

It does not verify an assertion, because there is no assertion. It answers a
request on a connection whose far end it selected. The verification is the
existing generation and fence check, which is about *freshness* — is this the VM
generation I think it is, and has it been superseded — not about *identity*.

### 3.5 Lifetime and revocation

The authority to spend on a broker slot lasts exactly as long as the operation
and its fence. It ends when:

- the operation settles, is cancelled, or times out at the catalog's `timeout_ms`;
- the fence advances (suspend, resume, or a new generation), which invalidates
  every in-flight frame;
- `agent_build` changes, which invalidates Brain's endpoint lease
  (`backend.rs:688-716`);
- the endpoint lease expires at 1800 s and is not re-minted;
- the session is paused, trashed or loses continuity
  (`crates/aex-session-domain/src/session.rs:54-84`);
- the organization is `AccountState::Paused`.

Nothing here is a bearer credential, so nothing needs a revocation list.

## 4. Why a customer holding the binary cannot impersonate another workspace

This is the central claim, so it is worth arguing rather than asserting.

Assume the strongest attacker permitted by the brief: workspace A's customer has
fully reverse-engineered `/opt/aex/tools/web_search` and `hands-agent`, is root
inside their VM, and can send arbitrary bytes on the broker socket and arbitrary
frames on the guest's HTTP port. They want a platform-paid call attributed to,
billed to, or returned to workspace B.

**They cannot construct a claim, because the message has no claim field.** A
broker slot carries a tool name and canonical arguments. There is no workspace id,
no session id, no organization id, no key, no signature. A tampered slot changes
*what is searched*, never *who is charged*. Contrast the sketched design, where
the message carries a credential and the entire security argument reduces to
whether that credential can be lifted — which, on a machine the customer owns, it
can.

**They cannot reach workspace B's broker, because they cannot reach anything.**
The guest has no outbound socket. This is not a firewall rule the root user can
flush; it is the absence of a TLS stack and an HTTP client in the binary, held in
place by a dependency-closure scan that fails if either appears. Even with
`NetworkPolicy::PublicInternet` and a hand-written raw socket, the target does not
exist: B's broker is not an endpoint. It is one end of a connection that Brain
originated to B's microVM.

**They cannot make Brain dial them on B's behalf.** Brain resolves the endpoint by
`GenerationId` from a generation head the platform wrote, and authenticates with a
token AWS mints for that one microVM. To be answered as B, the attacker would need
`GetMicrovm` and `CreateMicrovmAuthToken` on B's microVM inside AEX's own AWS
account. That is a compromise of the platform's account, at which point the
question is moot.

**They cannot replay.** There is nothing to capture. The broker socket is inside
their own VM, and its contents are their own query. Replaying it is calling the
tool again, which is a budgeted call, not an authentication bypass. Outside the
VM the only traffic is Brain-to-AWS-proxy HTTPS, bearer-authenticated by AWS, and
the customer is not on that path.

The residual risk is a platform bug: a generation head that names the wrong
workspace, or a Brain instance that answers a broker slot on a connection other
than the one the ticket was minted for. Those are ordinary correctness bugs,
reachable only by us, and testable — a receipt must self-attribute the same route
or it is a `ProtocolViolation` today (`router.rs:273-277`), and the same shape of
assertion applies to a broker reply.

## 5. What happens when the material leaks

There is no material, which is the point of the design. But "no material" should
be stated precisely, because the guest does hold *something*.

| What the guest holds | If the customer reads it |
| --- | --- |
| Generation id, session id | Public prefixed UUIDv7 identifiers (`gen_`, `ses_`). Not credentials. Knowing them authenticates nothing, because the only path that accepts them is the inbound one, and reaching it requires an AWS-minted token for that microVM |
| Image digest, bounds, root, size, protocol version | Non-secret configuration. Discloses the shape of the platform, not access to it |
| Broker request and reply bytes | The customer's own query and their own result |
| `X-aws-proxy-auth`, **if** AWS's proxy fails to strip it | A token scoped to *their own* microVM on port 8080 — that is, permission to talk to themselves. Blast radius is effectively nil |

That last row deserves a flag. **B5's guest half is declared, not earned.** The
falsifying test panics because nothing is deployed
(`tests/live/aex-live-hands-image/tests/boundary.rs:42-45`). The design should not
depend on the strip, and under this proposal it does not: even a guest that
observes the header gains only self-access. Under a bearer-token design it would
matter much more, which is another reason to prefer this one.

The honest worst case is not a leak at all. It is **a customer scripting the tool
in a loop**, which is not an attack — it is ordinary use of a tool they are
entitled to call. Nothing in §3 through §6 bounds it. §7 is where that is handled,
and it is the part of this proposal with the least existing support.

## 6. Where the platform credential lives, and why it never enters the guest

### 6.1 Custody

The platform's third-party key is loaded once by the process that spends it and
lives only in that process's memory. It must take the shape the repository already
uses for secrets, which is enforced by types rather than discipline — see
`ProviderApiKey` (`crates/aex-brain-provider-gateway/src/credential.rs:252`), an
`Arc<Zeroizing<String>>` with exactly one exit,
`sensitive_header()`, returning a `HeaderValue` with `set_sensitive(true)`, and a
compile-time negative-bound probe asserting it implements no serializer
(lines 1337-1351). `SecretPlaintext`
(`crates/aex-secret-domain/src/plaintext.rs:22`) states the principle: *"Plaintext
non-persistence is enforced by the **type**, not by discipline."*

### 6.2 The argument that it cannot enter the guest

Three independent reasons, any one of which is sufficient:

1. **No channel carries it.** The only bytes that cross into the guest are the
   run-hook payload — closed, sorted, eight keys, `deny_unknown_fields`, with a
   test that plants `apiKey` and `credentials` and proves they are refused — and
   protocol frames. A broker reply is validated against the tool's declared result
   schema, which for `web_search` is `{provider, query, results, truncated}`. There
   is no field a credential could occupy.
2. **The tool binary in the guest has no code that could use one.** It speaks a
   unix socket. It links no HTTP client. Giving it a key would require also giving
   it a TLS stack, which fails
   `the_binary_carries_no_tls_stack_it_could_reach_a_cloud_endpoint_with`.
3. **The spending process is not the guest.** The outbound call is made by
   `brain-mux`, which is where the key already is and where it stays.

The design should add a fourth, in the repository's own idiom: a `no_platform_key`
test on the broker reply type, mirroring `no_guest_billing.rs`, asserting the
reply schema's field set is closed and admits nothing secret-shaped.

### 6.3 An open question about which process holds it

If the key lives in `brain-mux`, then one process holds a **platform-wide** secret
alongside per-tenant secrets, while also processing untrusted content — tool
results, fetched pages, model output. A `brain-mux` compromise is already total,
so this does not widen the blast radius in kind, but it does put a platform-wide
credential in the noisiest process in the system.

The alternative is the owner's own instinct, correctly aimed: a small
`tool-broker` deployable holding only the platform credential, reachable only from
`brain-mux` over a private route, with an IAM policy that grants nothing else.
Cost: a new deployable, a new hop, added latency inside a 15 000 ms tool budget,
and one more thing to deploy and alarm on. Recorded as decision D-2 in §9.

## 7. Rate limiting, budget and abuse control

Nothing in §3 through §6 bounds spend. Two of the three bounds proposed here
extend machinery that already exists; the third has to be built. Each is named as
a function and a location rather than an intention.

The shape is three concentric bounds, because no single one is adequate:

| Bound | Horizon | Status today |
| --- | --- | --- |
| Per-session, via `Dimension::CostMicroUsd` | one session | **Built, empty** (§7.2) |
| Per-organization ceiling | a rolling window | **Does not exist** (§7.3) |
| Account state | a billing period | **Built, unreached** (§7.5) |

The per-session budget alone is not enough, because a customer opens as many
sessions as they like. The organization ceiling alone is not enough, because it
cannot stop one runaway session from consuming a whole organization's headroom in
minutes. The account state alone is not enough, because it moves at settlement
cadence.

### 7.1 Per-call admission — `brain-mux`, before the credential is exposed

A new port on the Brain application:

```
PlatformSpend::admit(&DispatchTicket, &ToolName) -> Result<SpendPermit, SpendRefusal>
```

**Enforcement point:** `ManagedWebExecutor::invoke`, immediately before the call
to `WebSearchCredentialSource::resolve`
(`crates/aex-brain-managed-web/src/executor.rs`). It must be before, so a refused
call costs no KMS operation and no vendor request. This mirrors the ordering the
custody path already uses, where a context mismatch is *"caught before a KMS call
is spent"* (`crates/aex-secret-aws/src/context.rs:78`) and the digest comparison
runs ahead of any decrypt
(`crates/aex-brain-provider-custody/src/authority.rs:174-176`).

`SpendPermit` should be the same shape of proof as `Grant<C>` in
`crates/aex-regional-http/src/capability.rs` — unforgeable by construction and
required as an argument by the function that spends — so that a future caller
cannot reach the credential without having passed the check. That is the same
trick `DispatchTicket` plays with `FenceGuard`, and it is what stops the bound
being a convention someone forgets to follow.

### 7.2 The per-session half — charge `Dimension::CostMicroUsd`

**This does not need building. It needs using.**

As established in §1.9, `aex_brain_domain::budget` already charges seven
dimensions up an agent tree, already halts with `FinishReason::Budget`, and
already carries `Dimension::CostMicroUsd` — a money dimension that has never been
consumed because *"under `BYOK` a model call does not touch this."*

**Enforcement point:** `consume(Dimension::CostMicroUsd, price)` on settlement of a
platform-paid tool call, in the same place `decide::settle_tool_call` records the
receipt (`crates/aex-brain-app/src/activation/run.rs:1683-1694`).

Three properties come free because the machinery is already correct: it is
conserved up the tree, so 256 subagents share one session's money rather than
getting 256 copies of it; exhaustion produces a terminal reason the model and the
customer can both see, rather than an opaque failure; and it is per session, which
is the granularity a customer actually reasons about.

Two things must be decided rather than inherited. `launch_session_grant` takes
`cost_micro_usd` as a parameter with no default, so somebody has to choose the
number — and the function's own doc says the constant *"is a constant only so a
test has a starting point; the Slice 11 load gates are the sizing authority."*
And the charge should be **reserved before the call and reconciled after**, not
charged only on settlement, or a session can overshoot by its full concurrency
width. `ProviderCalls` is granted `u64::MAX` and should stay that way; it is a
different dimension answering a different question.

### 7.3 The per-organization half — a conditional DynamoDB update

**Enforcement point:** a single-partition item per `(organization, window)` with
```
UpdateExpression:    ADD calls :one
ConditionExpression: attribute_not_exists(calls) OR calls < :ceiling
```
A failed condition is the refusal. This is the same shape the runtime activity
store already uses for the per-generation operation ceiling, so it is a known
pattern rather than a new one.

**Keyed by organization**, because that is the one key §7.2 cannot reach. The
session budget is conserved within a session and says nothing about a customer who
opens a thousand of them. `OrganizationId` is the billing and ownership boundary
(`crates/aex-wire/src/generated/ids.rs`), so it is the key where the bound and the
bill agree.

**One warning from the code.** The nearest existing thing to this counter is the
OTLP quota, and it is a cautionary tale. `services/regional-otlp/src/authority.rs:574-589`
runs `ADD reservedBytes …, reservedRecords …` on a `QUOTA#{workspace}` item with
**no `ConditionExpression`**. The numbers accumulate forever and are never compared
to anything, and `ErrorCode::TelemetryQuotaExceeded` is declared on three routes
and produced by none. A counter without a condition is telemetry wearing a
quota's name. The condition is the entire mechanism.

### 7.4 The ceiling — the limit registry, plus a check that it is read

A new `LimitId`, shape `map`, dimensions `{calls_per_minute, calls_per_day,
concurrent}`, added to `api/schemas/registries/limits.yaml` and
`limit-defaults.v1.json`. It rides the existing generated `LimitId` → capacity
defaults → `EffectiveLimits` plumbing, so no new configuration mechanism is
invented.

**With one addition, because the registry is demonstrably not enforcement.**
`telemetry.ingest_rate` has lived in that registry with published defaults and has
never been read by anything (§1.8). Adding a row is not adding a limit. The
proposal therefore includes a `aex-workspace-check` rule: **every `LimitId` whose
dimension names contain a rate must have at least one non-test reference outside
`aex-wire`, or the check fails.** That is what turns a declaration into a bound,
and it retroactively surfaces the `telemetry.ingest_rate` gap rather than letting
this design quietly join it.

### 7.5 The kill switch — the account-state gate, and the lever nobody pulls

The broker must consult the same account state the regional edge does, from the
session authority row Brain already holds. That part is cheap: the gate exists, it
is fail-closed on an unreadable state, and it terminates open leases as well as
refusing new requests.

**The gap is not the gate. It is that nothing writes its input.** As traced in
§1.8, `finance.billing_account.state` is inserted as `'active'` and never updated
by any production code path, the prepaid-credit `admit()` has only test callers,
and `spend_cap` bounds automatic top-up rather than spend. A customer whose balance
reaches zero keeps working.

For a customer-directed, platform-paid call path, that is the load-bearing defect,
and it is larger than anything else in this document. Two things are needed and
they are separable:

- **A writer for `finance.billing_account.state`**, driven by the settlement path,
  so that credit exhaustion produces `paused_top_up_required`. Everything
  downstream of that column already works, so this is the highest-leverage single
  change available. Recorded as **D-5**.
- **The two inner bounds** (§7.2, §7.3), because the account state is coarse and
  slow: it pauses an entire organization, it moves at settlement cadence, and it
  cannot express "this one tool has had enough".

One propagation detail worth recording, since a kill switch is only as good as its
latency. The state change and a `control.outbox_message` with topic
`account.state.changed` are written in the **same Aurora transaction**
(`migrations/central/20260801000500_baseline_finance.sql:319-321, 351-353`), so a
pause cannot commit without a path to the regions. `central-control-worker` then
drains that outbox on a schedule and writes the regional placement row. After
that it is immediate for new requests, because the edge never caches, and bounded
by `LISTEN_HEARTBEAT = 15 s` for an already-open stream
(`services/regional-observation-api/src/api.rs:62, 136`). **The drain cadence is
an EventBridge schedule supplied at deploy time and is not pinned in this
repository**, so the floor on pause latency is set by a deployment input rather
than by code. *(Inferred: total latency is drain interval plus one DynamoDB
write.)*

### 7.6 Metering — the counter exists and is deliberately unpriceable

`ObservabilityMeter::ToolInvocations` is exactly the right counter and is
structurally barred from carrying money (§1.8): disjoint from `Meter`, absent from
the `BTreeMap<Meter, Rate>` rate card, and refused by the outbox as
`Unrepresentable`. Pricing a platform-paid call therefore means a fifth `Meter` —
a wire change, a rate-card change, a rating change and a finance path change — or
deliberately breaking a separation the code was written to guarantee.

Until then a platform-paid `web_search` is **unbilled by construction**, and would
be unbilled anyway while `BillingMode::Shadow` is the default. Decision D-1.

Note the interaction with the platform's stated pricing posture, which is BYOK and
the customer pays their own provider. A platform-paid tool is a different product
shape, not a convenience toggle.

### 7.7 What is deliberately not proposed

No per-source-IP WAF rule: every call arrives from `brain-mux`, so a source-IP
limit would throttle ourselves. No Lambda reserved concurrency, per standing
policy. No in-process token bucket in `brain-mux`: it is a horizontally scaled
Fargate service with `desired_count = 2` and a per-process bucket bounds one
replica, which is the same per-instance-cache mistake `04ad0582` was written to
undo.

## 8. Alternatives, with what each fails at

### Option 1 — reuse the workspace API key inside the guest (the sketch)

**Reject.** Blocked before the security argument even begins: the platform does
not retain customer key plaintext and would have to start
(§2.1). Beyond that, the credential carries the whole workspace scope set, is not
bound to session, tool or time, is revocable only by breaking the customer's own
integrations, and does not function under `NetworkPolicy::None`. It also makes
every sandbox an authenticated client of the entire regional API surface rather
than of one tool, which is the guest-to-platform escalation the brief asks to
defeat, shipped as a feature.

*Fails at:* buildability, blast radius, revocation granularity, and the `none`
network posture.

### Option 2 — a session-scoped bearer token issued at launch

A sixth `CredentialKind` (`aex_gt_`), minted at generation launch, delivered in
the run-hook payload, bound to `(generation, session, workspace, organization)`,
carrying one scope, TTL bounded by the generation lifetime, verifier peppered and
projected onto a row the regional edge reads. A sixth `AssertionAudience` fits —
`AudienceSet(u8)` uses five of eight bits.

This is a real design and it would work. The costs are specific:

- `PresentedCredential` accepts one credential kind by deliberate invariant
  (`credential.rs:62-76`). It would accept two.
- A new audience and a new scope are a route-table change and a full contract
  regeneration, since `ScopeId` is derived from the route table.
- A new public endpoint that **every customer sandbox is expected to reach**. That
  is a new internet-facing authenticated surface, and it is the one place the
  guest-to-platform boundary becomes load-bearing rather than absent.
- The run-hook payload's closed eight-key set has to open to admit a secret,
  reversing boundary control **B4** ("no managed secret in the guest"), which is
  currently *earned* and asserted by a test that plants exactly such a key.
- Unavailable under `NetworkPolicy::None`.

*Fails at:* the thing it is bought for. The customer reads the token out of their
own guest and replays it from anywhere. Session-binding does not help, because the
customer owns the session and wants to spend on it. What it genuinely buys is
**scope confinement** — a leaked token spends one session's budget rather than
opening the workspace — and that is worth something. It is the right answer if and
only if the tool must run when Brain is not attached, which is the one case
Option 4 cannot serve.

### Option 3 — a per-call signed request the platform authorizes

The guest holds a signing key and signs `(tool, arguments, timestamp, nonce)`; the
platform verifies and admits.

Every cost of Option 2, plus two of its own:

- The customer now holds material that **mints** rather than material that **is**.
  This repository has already reasoned about that exact distinction:
  *"Holding the pepper and the verifier lets a holder check a presented key; it
  does not let them mint one"* (`credential.rs:15-19`). Handing a minting key to
  the party you are defending against inverts the property the whole credential
  design is built on. A leaked bearer token replays one request; a leaked signing
  key signs anything the grammar admits, forever, at machine speed.
- The nonce and timestamp defend against a third party who captures the wire.
  There is no such party. The wire is HTTPS from the customer's own machine to a
  public endpoint, and the customer *is* the client. The ceremony defends against
  nobody in this threat model while adding verification cost to a hot path.

The platform has just finished deleting a signed-assertion model for cost and
single-point-of-failure reasons (`04ad0582`). Reintroducing per-call signature
verification for a strictly weaker guarantee would be the same mistake with worse
ergonomics.

*Fails at:* everything Option 2 fails at, and additionally converts a leak from
"replay one shape" into "sign anything".

### Option 4 — the guest holds nothing; Brain brokers the paid leg (recommended)

*Fails at:* **detached and background execution.** Brain must be attached for the
duration of a brokered call. Any tool that must run while the activation is parked
cannot use this path. It also adds a protocol surface (a broker channel and a
guest-local socket), adds one round trip of latency inside the tool's timeout, and
puts a blocking socket client into a binary whose current virtue is that it links
no client of anything.

*Buys:* no new credential kind, no new audience, no new scope, no new public
endpoint, no new revocation surface, no reversal of B1, B4 or B6, no per-call
central round trip, and it works under `NetworkPolicy::None`. The identity is
positional rather than asserted, so there is no assertion to forge and no material
to leak.

### Summary

| | Guest holds | New public surface | Works with `network: none` | Reverses a boundary control | Leak blast radius |
| --- | --- | --- | --- | --- | --- |
| 1. Workspace API key | Full workspace credential | No (reuses regional edge) | No | B4; requires retaining key plaintext | Entire workspace, permanently |
| 2. Session bearer token | Scoped bearer | Yes, one per region | No | B4 | One session's tool budget, until expiry |
| 3. Per-call signature | Long-lived signing key | Yes, one per region | No | B4 | Unlimited signed requests, until rotation |
| 4. Broker (recommended) | Nothing | No | Yes | None | Nothing to leak |

**Recommendation: Option 4**, with Option 2 held in reserve and adopted only if
detached execution of a platform-paid tool becomes a requirement. If that happens,
adopt Option 2 for that tool alone rather than for the whole surface, so the new
public endpoint's exposure is proportional to the one thing that needs it.

**And one option cheaper than all four, which should be rejected explicitly rather
than forgotten.** Keep `web_search` as `ToolBoundary::ManagedWeb`. It never enters
the guest, the router already sends it to the `ManagedWeb` executor slot, and it
needs no broker, no protocol change, no guest-local socket and no new identity
mechanism of any kind. Every spend control in §7 still applies unchanged, because
they all live on the Brain side. What it costs is the uniformity the larger
proposal is after: `web_search` would not be an executable the agent can pipe into
a shell, and the tool surface would stay split between two kinds of thing.

That is a real cost and it may well be worth paying. It is worth deciding on its
merits rather than by default, because the security properties of Option 4 and of
this option are identical — the difference is entirely about how the tool feels to
the model, not about who can spend the platform's money.

## 9. Decisions this document could not settle from the repository

**D-1. Does `web_search` become platform-paid at all?**
The repository's pricing stance is BYOK, and it is stated flatly rather than
implied — `references/rewrite/providers.md:62-65`: *"AEX does not provision a
shared or managed gateway key."* Every credential path in the tree is
workspace-scoped, and the `Meter` / `ObservabilityMeter` split exists specifically
so a per-call counter cannot acquire a price. A platform-paid tool runs against
the grain of all of that, which is a reason to decide it deliberately rather than
discover it.

*Options:* (a) **stay BYOK** and fix the advertisement gap at `wake.rs:791` so the
tool appears when a customer has admitted the secret — cheapest by an order of
magnitude, and worth noting that the tool is fully implemented and merely never
advertised, so this is a small change with a large visible effect; (b)
**platform-paid, metered**, which means the fifth `Meter` and everything
downstream of it; (c) **platform-paid, capped, unmetered** — a small allowance
absorbed as COGS with a hard ceiling and no invoice line, cheapest to ship, no
per-customer attribution, and the option that quietly turns a product decision
into a cost centre nobody watches.

**D-2. Does the platform credential live in `brain-mux` or a separate broker
deployable?** In `brain-mux`: no new deployable, but a platform-wide secret sits
in the process that also handles untrusted tool results and model output. Separate
broker: proper isolation and a minimal IAM policy, at the cost of a deployable, a
hop, and latency inside a 15 000 ms budget.

**D-3. Where is the platform credential stored at rest?** Secrets Manager, with
`crates/aex-central-aws/src/pepper.rs:215` as precedent; or the regional secret
custody path with a reserved platform workspace, which reuses the KMS
encryption-context discipline but places a platform key in a tenant-shaped table
whose IAM grants are written for tenant access.

**D-4. Organization or workspace for the spend ceiling?** Organization is the
billing boundary and the only key where the bound and the bill agree. Workspace
contains blast radius better — one runaway workspace does not starve its siblings.
Both are defensible; a two-level ceiling is a third option and costs a second
conditional update per call.

**D-5. Who writes `finance.billing_account.state`?** This is the highest-leverage
open item and it is not really a decision about this design — it is a defect this
design happens to depend on. The pause gate, its view, its fail-closed absence
handling, its regional projection and its lease termination are all built and
correct. The column that feeds them is written once, as `'active'`, and never
again. Until something writes it, credit exhaustion halts nothing and the only
answer to a runaway customer is a human noticing. *Options:* (a) the settlement
worker writes it when available credit crosses zero; (b) a separate reconciler
evaluates it on a schedule, which adds lag but keeps settlement single-purpose;
(c) leave it and rely solely on the per-call ceiling in §7.1, which bounds a loop
but not a month.

**D-6. What does the model see at the ceiling?** A typed refusal the model can
route around is friendlier and matches the standing prelaunch-friction policy, but
it is also a probe a hostile customer can use to map the budget. A silent failure
is worse under every reading. *Recommendation:* typed refusal, with the remaining
budget deliberately absent from the message.

**D-7. Does a brokered call survive suspend and resume?** A guest process blocked
on the broker socket across a snapshot is a state the operation machine does not
currently describe. Options: refuse to suspend a generation with an open broker
slot; or fail open slots on resume and let the tool report a retryable error.

**D-8. Is a guest-local unix socket acceptable at all?** It is inside the guest, so
it is not a trust boundary and root can do anything to it. But it is new surface in
`hands-agent`, whose current interface is exactly one TCP port, and
`references/rewrite/hands.md` treats that singularity as a property worth stating.
An alternative is a well-known file plus inotify, which is worse. Recording it so
the decision is made rather than absorbed.

## 10. Verified versus inferred

**Verified in code**, with the citations above: the credential-free guest and its
three environment variables; the closed eight-key run-hook payload and its
`deny_unknown_fields` decoder; the absence of any `execution_role_arn` field; the
dependency-closure scans forbidding a TLS stack or cloud SDK in the guest binary;
the inbound-only five-verb protocol, its headers, its generation and fence checks
and the `agent_build` stamp; the AWS-minted, port-scoped, 1800-second endpoint
token and its memory-only custody; the two network modes and the absence of any
resolver that picks one; the public ALB admitting `0.0.0.0/0:443`; `web_search` as
BYOK against `aex_web_search`, implemented but never advertised because
`ResolvedSecretNames::default()` is hardcoded empty; the ten-stage regional edge,
its uncached per-request projection read and its peppered MAC; the fact that only a
verifier is stored and never a key plaintext; `PresentedCredential` accepting one
credential kind; `AudienceSet` using five of eight bits; `DispatchTicket` and its
`FenceGuard`-gated `mint`; the four resource-shaped meters and the absence of a
count meter; `EffectiveLimits` being four size bounds; the three zero-dollar
`ObservabilityMeter` counters and their structural disjointness from `Meter`;
`BillingMode::Shadow` as the default; the seven-dimension session budget with
`CostMicroUsd` never consumed and `ProviderCalls` granted `u64::MAX`; the absence
of any request-rate limiting outside one WAF rule on unauthenticated device-flow
routes; `telemetry.ingest_rate` being declared with defaults and read by nothing;
the OTLP quota counter accumulating with no `ConditionExpression`; the
complete and fail-closed account-state gate from `finance.billing_account.state`
through the `account_state_v1` view and the control projection to edge stage 8 and
lease revalidation; and the fact that no production code path writes that column
after `ensure_account` inserts `'active'`, that the prepaid-credit `admit()` has
only test callers, and that `spend_cap` guards automatic top-up rather than spend.

**A correction worth recording**, because the first pass of this document got it
wrong and the error was the flattering kind. It is not true that the platform has
"no budget object and no credit balance". It has a chart of accounts, a prepaid
liability split into available and reserved, an admission function that blocks on
exhaustion, a spend cap, and four runaway guards on automatic recharge — a more
complete finance model than most systems at this stage. What it does not have is a
single writer connecting any of that to the column the enforcement path reads.
"The mechanism is missing" and "the mechanism is built but unreached" call for
opposite work, and only the second one is true here.

**Inferred**, and flagged where it matters: that `NetworkPolicy::None` means a NIC
without an egress route rather than no NIC, since the ingress connector is attached
unconditionally and the guest binds `0.0.0.0:8080`; that the intended launch
posture is `none`, from fixtures and the public architecture example rather than
from any default in the type system; that a `public_internet` guest can reach the
platform's public hostnames, since nothing filters guest egress and those hostnames
are on internet-facing listeners; and that `brain-mux` runs on Fargate, read from
`release/units.toml` rather than from an instantiated Terraform resource.

**Unproven by the repository's own admission.** Every live boundary probe that
would settle guest-side reachability is marked *unearned, declared* and panics on
invocation: `b1_the_guest_reaches_no_instance_credential`,
`b2_the_guest_reaches_no_private_aex_route`,
`b5_the_guest_never_observes_the_endpoint_auth_header`,
`b9_two_generations_cannot_see_each_other`
(`tests/live/aex-live-hands-image/tests/boundary.rs`). This proposal is written so
that none of its guarantees depends on B5 holding, which is deliberate: a design
that needs an unearned control is a design that needs a deployment before it can be
believed.

**Also worth recording, since it bears on scheduling.** No production code path
creates a session or allocates a generation today —
`TransactionIntent::CreateSession` is refused at
`crates/aex-session-app/src/plan.rs:661-664` with the reason stated, and
`create_generation` has only test callers. `infra/` contains no IAM policy for
`lambda:RunMicrovm` and no Terraform that deploys `brain-mux` or
`runtime-control-worker`. The mechanism proposed here is therefore designed
against a launch path that is not yet reachable in production, which makes it
cheap to adopt now and expensive to retrofit later.
