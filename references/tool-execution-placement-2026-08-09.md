---
title: Where tool work executes, and how attribution survives the move out of the Brain
description: >
  Reconciles three disagreeing design records into one placement answer: a
  dedicated tool executor outside `brain-mux`, called only by the Brain and never
  by the sandbox; why that keeps identity positional rather than asserted; why
  the compute shape is not the identity question; what batching costs and what it
  currently breaks; whether `network: none` still permits `web_search`; and where
  each missing bound is enforced.
keywords:
  - tool execution
  - placement
  - sandbox identity
  - impersonation
  - dispatch lane
  - parallel tool calls
  - network policy
  - rate limiting
audience: owner, architect, implementation agents
status: proposal; §8 steps 1-3 implemented 2026-08-10
date: 2026-08-09
related:
  - references/sandbox-platform-identity-2026-08-09.md
  - references/rewrite/hands.md
  - references/rewrite/tools-mcp.md
  - references/architecture.md
  - crates/aex-brain-tool-catalog/src/manifest.rs
  - crates/aex-brain-app/src/activation/run.rs
  - crates/aex-identity-domain/src/assertion.rs
  - crates/aex-hands-control-aws/src/provider.rs
  - runtimes/brain-mux/src/wake.rs
---

# Where tool work executes

Four documents now describe tool execution and three of them disagree about
where it happens. This record settles the placement question so that the next
reader has one answer, and it is written against the owner's decision that a
dedicated tool-handling task — not `brain-mux` — executes tool work.

Everything marked **verified** was read in this repository at commit `f0661f41`.
Everything marked **inferred** is reasoning from what was read and is flagged
where it carries weight.

## 0. Reconciliation — what survives

Two of the prior records live in the outer workspace repository rather than in
this one. Paths are given in full so the next reader can find all four.

| Record | Status | On what points |
| --- | --- | --- |
| `../references/managed-web-tool-execution-2026-08-09.md` | **Superseded on placement** | Its §3/§5/§6 conclusion — keep execution in `brain-mux` — is overridden by owner decision. Its findings survive intact: the end-to-end managed-web trace, the `egress.rs` SSRF account, the batching finding, defect 1.4d (the shared permit lane), the verified absence of rate limits, domain policy and spend caps, and the redaction verdict. Nothing in it was wrong; it answered a narrower question than the one now being asked. |
| `../references/tool-execution-framework-2026-08-09.md` | **Accepted shape, one section superseded** | Tools as executables, the §5 invocation contract, the §6 identity/versioning layers, the §9 delivery resolution and the §11 seven gaps all stand and this record builds on them. Its **§8 is superseded**: the tool must not be an HTTPS client that presents a per-call proof to a platform endpoint (§3.4 below gives the attack). Its **D1 is settled by the owner** — `web_fetch` moves into the sandbox and fails honestly without egress. Its "independently deployable" tension is closed by the owner withdrawing the claim, which lands where its own §9.3 already recommended. |
| `references/sandbox-platform-identity-2026-08-09.md` | **Argument survives, conclusion superseded** | Its §1 findings, its §2 rejection of a workspace key in the guest, its §8 comparison of the four identity options, and its §7 enforcement-point map are adopted here unchanged. Its **conclusion that `brain-mux` must make the call itself is superseded**, and so is its §3.1 broker channel (guest-local unix socket, `broker_reply` verb). The property it was protecting — **identity is positional, so nothing is claimed and nothing can be forged** — is preserved by a different mechanism, and §3 below shows the property does not depend on who executes. |
| this record | new | Adds the missing axis: **placement is a declared property of a tool, not a per-tool argument**, and the catalog already declares it. |

**The one sentence a later reader needs:** the identity record's security argument
does not actually rest on the Brain executing the work. It rests on the guest
never being the caller. Moving execution to a separate service is compatible with
it; letting the sandbox call that service is not.

---

## 1. The decision

1. **A dedicated tool executor runs platform-destination tool work.** Not
   `brain-mux`. Its callers are the Brain and nothing else.
2. **The sandbox runs sandbox-destination tool work**, as executables in the
   image, per the accepted framework.
3. **Placement is derived from a property the catalog already declares.**
   `EgressClass` has exactly the four arms needed (`manifest.rs:351-360`):
   `None`, `ManagedInternet`, `McpRemote`, `GuestInternet`. `ManagedInternet`
   means the destination is fixed by the platform and paid by the platform, so it
   runs on the executor. `GuestInternet` and `None` run in the sandbox.
   `web_fetch` becomes `GuestInternet` when it moves; `web_search` stays
   `ManagedInternet`.
4. **The same binary, the same contract, either host.** The executor spawns the
   tool executable with the framework's stdin/stdout contract, exactly as the
   guest does. One artifact, one result document, one bounds implementation.
5. **Attribution never passes through the guest**, so there is no claim for a
   reverse-engineered binary to forge. The Brain is the only party that states
   whose call it is, to a service that accepts that statement from nobody else.

The user-facing rule that falls out of (3), and it is short enough to put in the
docs verbatim:

> A tool that talks to a destination the model chose uses the session's network.
> A tool that talks to a destination the platform fixed uses the platform's.

That is why `web_fetch` fails under `network: none` and `web_search` does not.
It is predictable, it is one sentence, and it is already encoded in the catalog.

---

## 2. Where the work executes

### 2.1 The finding that reorders the options

**The compute shape is not the identity question.** The identity question is
answered entirely by *who calls the executor*, and every option below answers it
identically as long as the answer is "the Brain". Latency, idle cost and
operational weight are what actually separate them.

The sharpest form: **a Lambda invoked through `lambda:InvokeFunction` by the
`brain-mux` task role has no network listener at all.** There is no port for a
guest to reach, in principle or in practice; IAM is the entire admission control.
On the identity axis that is *stronger* than an ECS service behind any listener.
The reason to prefer ECS is latency and connection reuse, not security.

### 2.2 The options

| | Latency added to a blocked turn | Isolation from `brain-mux` | Idle cost | Identity |
| --- | --- | --- | --- | --- |
| **A.** In-process in `brain-mux` (superseded) | zero | none — this is the defect | none | positional, trivially |
| **B.** Dedicated executor service, called by Brain (**recommended**) | one in-VPC round trip; *inferred* 1–3 ms | full: separate process, separate task role, separate IAM | a standing Fargate floor — the placement policy's own figure is ~$72/month at two tasks | positional at every hop the customer can touch |
| **C.** Lambda per tool class, invoked by Brain | *inferred* 10–30 ms warm, 100–300 ms cold, plus a fresh vendor TLS handshake per cold instance | full, and with no listener at all | zero | **strongest** — no listener exists |
| **D.** Sandbox tool calls the executor over HTTPS (framework §8) | start + a 250 ms poll floor growing to 4 s + a result pull | full | as B or C | **broken** — see §3.4 |
| **E.** Guest asks, Brain relays to B or C (identity record §3.1) | as D, plus a broker wait | full | as B or C | positional, but at a heavy price — see §6.3 |

Sources: poll growth 250 ms → 4 s is `crates/aex-brain-hands/src/backend.rs:86-92`
(**verified**); the $72 floor is `../references/compute-placement-policy-2026-08-09.md`
condition 4 (**verified**, its number not mine); the millisecond figures are
**inferred** and unmeasured.

**A caveat on leaning on that policy document.** It is `accepted`, and it is
already stale in one place: its condition 5 cites
`crates/aex-regional-http/src/assertion.rs` for a 30-second verified-assertion
cache, and that file no longer exists — `04ad0582` deleted it, so its verdict on
`central-authz` ("every regional request hits it on a 30 s cache miss") is no
longer true. Nothing in this record depends on that paragraph; the rule and
condition 4 are unaffected. Flagged because the next person to cite it should
know.

### 2.3 Recommendation, and the honest tension in it

**B is the destination. C is an acceptable first deployment of the same code.**

The placement policy's rule 1 — "nothing on the frequent user request path runs
on Lambda" — argues for B, and a tool call the model is blocked on is exactly the
frequent user request path. Its condition 4 — idle cost is a real input — argues
for C, and it argues hard, because `web_search` is **advertised to nobody today**
(`wake.rs:791` hardcodes `ResolvedSecretNames::default()`, **verified**) and
`web_fetch` is a handful of calls per session. Standing up a fourth Fargate
service for a surface with no traffic buys a monthly bill and a deploy unit
before it buys a user anything.

The hedge is real rather than a dodge: the executor is one crate with one
handler, and `units.toml` distinguishes `rust-lambda` from `rust-oci-service` by
a `kind` field and a `[unit.fargate]` block. The identity design, the quota
enforcement and the request shape are byte-identical between them. Shipping as
`rust-lambda` now and promoting to `rust-oci-service` when call volume justifies
the floor costs one registry edit and no design change.

**The Terraform cost is not symmetric, and it favours C more than the table
suggests.** `infra/modules/ecs-service` requires a `target_group_arn` and
load-balancer security groups, and the only ALB module in the tree is
`alb-public`, which hardcodes `internal = false`
(`infra/modules/alb-public/main.tf:48`, **verified**). A private-only service
therefore needs a new internal-ALB module or Service Connect support in
`ecs-service`. A Lambda invoked by the Brain needs neither: no listener, no
target group, no load balancer, one `lambda-function` module instance and one
`lambda:InvokeFunction` statement on the `brain-mux` task role. Both shapes need
new Terraform — `infra/examples/region-application` today deploys only the ECS
cluster and `session-stream-api`, with zero `lambda-function` instances
(**verified**) — but C's is materially smaller.

**And `brain-mux` does not autoscale.** Its Terraform forces autoscaling bounds
equal to `desired_count` and mandates `AEX_MAX_ACTIVE_ACTIVATIONS = "16"`
(`infra/modules/ecs-service/variables.tf:102-123, 276-307`, **verified**), so the
production ceiling is a fixed 2 tasks × 16 activations. Whatever the executor's
shape, it must not become the thing that makes that ceiling bind.

### 2.4 What "out of the Brain" does and does not buy

The owner should not be told this is a bigger win than it is.

**It buys**, and these are worth having:

- The vendor credential leaves the process that parses fetched web pages, model
  output and tool results. That is a strict custody improvement and it is the
  best single argument for the split.
- The fetch's CPU, its sockets, its TLS handshakes and its HTML conversion leave
  a 2 vCPU / 4 GiB task (`units.toml`, **verified**) that also holds activations,
  model streams, leases and the journal.
- A bound that survives a Brain bug: the executor re-checks quota itself and does
  not take the Brain's word for it (§7.1).
- It makes intra-batch concurrency affordable later (§4.4), which co-residency
  does not.

**It does not buy**, and claiming otherwise would be false:

- **Any reduction in the Brain's authority.** `brain-mux` already mints
  `DispatchTicket`s for every tenant, resolves every tenant's BYOK credential and
  owns the journal. It remains the only party that can say whose call this is,
  and it must. A compromised Brain can still cause and misattribute platform-paid
  calls. What it can no longer do is exfiltrate the vendor key.
- **Release of the activation.** The Brain still holds the activation, the lease
  and a lane permit while it waits on the executor. The *work* leaves; the *wait*
  does not. Removing the wait means making a platform tool call a
  `DurableDetached` effect so the activation parks — the arm exists
  (`effect.rs:61-76`) and the park path exists (`run.rs:1696-1740`), both
  **verified** — at the cost of the executor needing a durable operation store, a
  result-query path and recovery. That is a real upgrade, not a day-one
  requirement, and it is decision **P-4**.

---

## 3. Attribution without a forgeable claim

### 3.1 The rule

**Attribution is derived at every boundary the customer can touch, and asserted
only across boundaries the customer cannot reach.**

Three hops, and each is a different kind of thing:

| Hop | Kind | Why it holds |
| --- | --- | --- |
| model → Brain | derived | The tool call arrives inside an activation the Brain already owns under a `FenceGuard`. Nothing is presented. |
| Brain → executor | **asserted**, cryptographically | The Brain signs a short-lived envelope naming organization and workspace. The customer cannot mint one, and cannot reach the executor at all — under C there is no listener, under B the listener is private. §3.2. |
| executor → vendor | platform credential | Compile-time-constant destination, key never leaves the executor's memory. |

The guest appears nowhere in that chain. That is the whole design. The identity
record's central property — *nothing is claimed, so nothing can be forged* —
survives because it was never a property of the Brain doing the work; it was a
property of the guest not being the caller.

### 3.2 What the Brain passes, and how the executor verifies it

**Reuse the assertion envelope that already exists.** `aex_identity_domain::assertion`
is a fixed-layout 323-byte Ed25519 envelope with no algorithm negotiation, no
variable-length field and no allocation; `verify` re-checks
`expires_at_ms - issued_at_ms <= 30_000` and refuses a validly signed assertion
that claims longer, so an issuer bug cannot lengthen the window; the signature is
checked before every semantic check except length, magic, version, algorithm and
`kid` (all **verified**, `crates/aex-identity-domain/src/assertion.rs:1-40`). It
already carries `organization_id`, `workspace_id`, `account_state`, `scopes`,
`audience_service` and a `Plane` discriminant.

`AssertionAudience` has five arms and `AudienceSet` is a `u8` bitset, so three
bits are free (**verified**, `crates/aex-internal-contracts/src/assertion.rs:47-128`).
Declaration order is wire order, so appending a sixth audience is the safe
direction.

The request the executor accepts:

```rust
struct ToolExecRequest {
    assertion:      IssuedAssertion,   // audience = ToolExec
    tool:           ResourceName,
    manifest:       ContentHash,       // framework §6.3 layer 3
    arguments_jcs:  Vec<u8>,           // canonical, already schema-validated
    effect:         EffectId,
    attempt:        u16,
    deadline_ms:    u32,
}
```

**The type has no workspace field, no organization field and no principal field.**
That is not an omission to be tidied later; it is the mechanism. The executor
resolves the tenant from the signed envelope and from nowhere else, exactly as
the broker slot in the identity record's §4 carried a tool name and arguments and
no principal. Pin it the way the repository already pins this shape: a
`no_principal_in_request` test asserting the request type's field set is closed
and names nothing tenant-shaped, mirroring `no_guest_billing.rs`.

Verification order at the executor, and the order is the point — each step is
cheaper than the next and a refusal must never reach a paid operation:

1. The request arrived through the only channel that exists — an IAM-scoped
   invoke under C, a private listener under B. (Positional; §3.3.)
2. Envelope signature verifies against the key set; unknown `kid` refused.
3. `audience_service == ToolExec`, `Plane` matches this plane, region matches.
4. Not expired; declared lifetime ≤ 30 s.
5. `account_state == active`, fail-closed on anything else.
6. Tenant resolved **from the envelope**.
7. Quota admitted by conditional update (§7.3).
8. Only now is the vendor credential touched.

That ordering mirrors the custody path's own discipline, where a context mismatch
is *"caught before a KMS call is spent"* (`crates/aex-secret-aws/src/context.rs:78`,
**verified**).

**Where the signing key lives.** In `brain-mux`, as a new `kid` in the
verification key set — **not** minted per call by `central-authz`. Commit
`04ad0582` deleted exactly that shape for being a synchronous central invoke and
a single point of failure for every regional service in every region at once; a
per-tool-call central round trip would be the same mistake with a worse hot path.
A locally held signing key has no such cost.

The interface to reuse is already written and already deployed:
`AssertionSigner` with `LocalSigner`, the private key unwrapped once per cold
start and held in process (`crates/aex-identity-domain/src/assertion.rs:348-377`,
**verified**). Note also that the same module records why KMS asymmetric signing
was rejected — *"a per-request KMS asymmetric `Sign` would add round-trip latency
and a five-figure annual charge for no extra protection over an artifact that is
internal, audience-bound, credential-bound and lives thirty seconds"* — which
applies here verbatim.

Worth recording while touching this: **`central-authz` is currently orphaned on
the regional path.** `crates/aex-regional-http/src/authz.rs:26` is a section
header reading *"There is no `central-authz` client here"*, all four regional
service configs say the same, and yet the unit still carries
`reserved_concurrency = 200` (`units.toml:194-197`), all **verified**. A working
short-lived-token minting facility with no consumer is exactly the substrate this
design needs, and its idle reservation is worth revisiting separately.

**What a compromised Brain yields, stated plainly:** everything it yields today,
plus the ability to sign an assertion for any workspace. Since `brain-mux`
already resolves any tenant's BYOK credential and mints `DispatchTicket`s for any
agent it holds, the signing key adds no authority it did not have. The delta runs
the other way: after the split, a compromised Brain **cannot read the platform's
vendor key**, because that key is in a different process with a different task
role. Brokering does not reduce the Brain's authority. It reduces the Brain's
custody. Those are different words and only the second one is earned here.

### 3.3 Why a reverse-engineered tool binary cannot reach the executor

Assume the strongest attacker the brief allows: root in their own microVM, the
tool binary disassembled, `hands-agent` disassembled, arbitrary bytes on any
socket they can open.

- **The guest has no outbound socket under `network: none`.** `RunMicrovm` is
  built with `egress_network_connectors: Vec::new()` for `NetworkPolicy::None`
  (**verified**, `crates/aex-hands-control-aws/src/provider.rs:238-251`, golden
  test at `:428-445`). No internet, no AWS APIs, no IMDS, no package registries.
- **Under `public_internet` the guest can reach public hostnames but not private
  routes.** Boundary control B2 is *"no AEX VPC egress or private route"*. The
  executor has no public listener under B or C, so there is no name to resolve
  and no address to reach.
- **B2 is declared, not earned.** Every live boundary probe that would prove it
  panics because nothing is deployed
  (`tests/live/aex-live-hands-image/tests/boundary.rs`, **verified**). **This
  design therefore does not depend on it.** Step 2 of §3.2 is a cryptographic
  gate that holds even if the network gate does not: the guest holds no signing
  key, no key material of any kind, and the binary links no TLS stack at all
  (`the_binary_carries_no_tls_stack_it_could_reach_a_cloud_endpoint_with`,
  **verified**).
- **The guest cannot make the Brain dial somebody else's VM.** The endpoint comes
  from `GetMicrovm` in AEX's own account and the proxy token is AWS-minted,
  scoped to one microVM and to port 8080 only (**verified**,
  `crates/aex-hands-control-aws/src/aws.rs:162-194`,
  `crates/aex-brain-hands/src/guest.rs:55-61`).

One fact worth carrying, because it is easy to get wrong: **the guest's own HTTP
server is loopback-reachable from inside the VM and has no local authentication**
— it binds `0.0.0.0:8080` and the router applies no auth layer, because the AWS
proxy is the auth boundary (**verified**, `crates/aex-hands-agent/src/boot.rs:34-39`,
`runtimes/hands-agent/src/serve.rs:233-266`). A customer's process can POST to
`127.0.0.1:8080` and drive the five verbs. This buys them nothing outside their
own VM — no verb makes the platform do anything and none emits a network request
— but it is the reason a broker built on that port would need care, and it is one
more reason not to build one (§6.3).

### 3.4 Why the sandbox must not call the executor directly — the attack

The framework record's §8 shape is: the tool is an HTTPS client, reads a per-call
proof from `Exec.env`, and calls a platform endpoint. Its own §8.2.3 concedes the
decisive fact — *the environment of a process is readable via `/proc/<pid>/environ`
by the customer, who is root, so the proof is disclosed by design*.

Follow that through:

1. The proof is a **bearer credential**. The customer extracts it in the first
   second and can replay it from anywhere on the internet until it expires.
2. The endpoint must therefore be **internet-facing and authenticated**, because
   every customer sandbox has to reach it. The platform's vendor key now sits
   behind a public authenticated API whose credential is handed to the party the
   threat model treats as hostile.
3. The executor's only defence is the quota keyed to the proof. That is precisely
   the identity record's Option 2, whose verdict was: it *fails at the thing it is
   bought for*.
4. It **does not work at all under `network: none`**, which is the intended launch
   posture.
5. And it is worse than Option 2 in one respect Option 2 avoided: the framework
   shape puts the proof on a path that must also carry the arguments, so the
   endpoint is both the spend authority and the argument sink.

Cross-workspace impersonation is *not* the failure. The customer cannot mint
workspace B's proof, so they cannot spend B's budget. The failure is that a
design which had **nothing to leak** is exchanged for one whose entire security
argument is the expiry time on a credential the customer holds by construction.
That is a strict regression against the property the owner has already accepted,
and it buys nothing that §3.2 does not give for free.

**Reject.** If a sandbox-resident tool ever genuinely must reach a platform
resource, §6.3 prices the only shape that preserves the property.

---

## 4. Serial versus parallel

### 4.1 What the dispatch model permits today

**Verified**, and the prior finding is confirmed and extended.

`crates/aex-brain-tool-catalog/src/router.rs:186-190`:

```rust
let parallel_safe = advertised.entries.iter().all(|entry| {
    entry.descriptor.effect == EffectClass::Pure
        && entry.descriptor.determinism == Determinism::Deterministic
        && entry.descriptor.bounds.concurrency_weight == 0
});
```

and `crates/aex-brain-app/src/activation/run.rs:94-96`:

```rust
let parallel = !advertised.definitions.is_empty()
    && advertised.parallel_safe
    && model.capabilities().has(Capability::ParallelTools);
```

`web_fetch` is `NonReplayable`, weight 4 (`catalog.rs:450-464`). `Determinism` is
*derived* from `EffectClass` (`catalog.rs:175-181`), so the first two conjuncts
say the same thing twice and `concurrency_weight == 0` is the only independent
axis. The predicate is over the **whole advertised catalog**, so one non-pure tool
disables batching for every tool. With the live surface — `todo_read`,
`todo_write`, `web_fetch` — `parallel_safe` is false, pinned by a test at
`runtimes/brain-mux/src/wake.rs:1416-1423`.

**And the Brain's driver is sequential regardless of the flag.** The planner
returns a `Vec<PendingCall>` and the driver ignores the vector:

```rust
OwedStep::ToolCalls { calls } => {
    if calls.is_empty() { return Ok(Step::Nothing); }
    self.tool_call(None).await
}
```

`run.rs:1088-1126`; `tool_call` runs *"the first outstanding tool call"*
(`run.rs:1557-1569`), selected by the assistant message's tool-use order so *"a
slow tool cannot reorder a turn"* (`run.rs:2135-2144`). There is no `join_all`,
no `FuturesUnordered` and no `tokio::spawn` anywhere in the Brain crates. All
**verified**.

### 4.2 The live defect this predicate is causing

**Verified, and it is larger than "no batching".** Four of the six provider
dialects **refuse to build a tool-bearing request when `parallel_tools` is
false**, because their wire surface has no field that expresses "one tool at a
time":

```rust
// crates/aex-brain-provider-gateway/src/google.rs:659-663
if !view.parallel_tools {
    return Err(RequestBuildError::Encoding {
        reason: "google has no field that forbids parallel function calls",
    });
}
```

```rust
// crates/aex-brain-provider-gateway/src/deepseek.rs:525-533
} else {
    // The chat-completions surface has no `parallel_tool_calls` switch, so
    // "one tool at a time" cannot be expressed. Refused rather than dropped.
    return Err(RequestBuildError::SamplingUnsupported { field: "parallel_tool_calls" });
}
```

The same refusal is in `zai.rs:316-320` and `moonshotai.rs:780-786`. Only
Anthropic (`disable_parallel_tool_use`) and OpenAI (`parallel_tool_calls`) can
express it.

**So: advertising `web_fetch` makes every tool-bearing turn on DeepSeek, Google,
Z.AI and Moonshot fail at request build.** Refusing rather than silently dropping
is the right instinct and the comments say so; the problem is upstream, in a
predicate that sets the flag false for reasons that have nothing to do with
whether the model may emit two calls at once.

Whether a customer hits this depends on the model chosen, and the refusal is a
build-time `Err` rather than a provider error, so it would surface as a dispatch
failure rather than a `400`. The standing convention that release-gating tests
run DeepSeek only makes this worth checking first: either the gating suite never
advertises a non-pure tool, or it is red on a path nobody has read. *That
convention is recalled rather than verified in this checkout — confirm it before
relying on the inference.*

### 4.3 What to change

**Split the property.** The current predicate conflates three questions:

| Question | Currently answered by | Should be answered by |
| --- | --- | --- |
| May the model *emit* several tool calls in one message? | `parallel_safe` | model capability alone |
| May *we* run two calls concurrently? | `parallel_safe` | a per-batch policy over descriptors |
| May a call be *replayed* after an ambiguous failure? | `EffectClass` / `RecoveryClass` | unchanged — already correct |

Concretely, `parallel` becomes:

```rust
let parallel = !advertised.definitions.is_empty()
    && model.capabilities().has(Capability::ParallelTools);
```

This is safe **today, with no other change**, because §4.1 establishes that the
driver executes one call at a time in emission order behind its own durable
prepare/commit whatever the flag says. The flag has never controlled our
concurrency; it only ever controlled the model's emission.

`concurrency_weight` keeps its real job — charging the lane — and becomes the
input to a *per-batch* execution policy when concurrency is actually built.

**Is it worth it?** Yes, and not mainly for concurrency. A batch of three
`web_fetch` calls today costs three assistant messages, which is three model
round trips and three context prefills. Serial execution of one batch costs one.
**The expensive thing is the turn, not the tool.** The turn saving is available
immediately at zero concurrency risk; the concurrency saving is not, and should
not be taken first (§5).

Two bounds to check while doing it, both **verified** to exist and neither traced
against a batch: `DEFAULT_MAX_TOOL_CALLS: u16 = 128` per turn
(`aex-brain-provider-gateway/src/budget.rs:33`) and `max_steps_per_turn`
(`aex-brain-domain/src/planner.rs:178`). Each call in a batch consumes a step, so
a wide batch can exhaust the per-turn step budget in a way a single call never
did.

One consequence to accept deliberately: `parallel_tools` participates in the
canonical request digest (`aex-model-catalog/src/canonical.rs:679, 710`,
**verified**), so flipping it changes every `request_hash`. Prelaunch that is
free; it will not be later.

### 4.4 The execution policy the owner asked for

The Brain should dispatch a batch under a declared policy, and the policy should
have exactly two values:

- **`Serial`** — the default and, today, the only implementable one. In emission
  order, each behind its own effect.
- **`Concurrent { max_in_flight }`** — permitted only when every call in *that
  batch* declares it, evaluated per batch rather than over the whole catalog, and
  gated on §5 being fixed first.

Making the policy per-batch rather than per-catalog is the substantive change.
The current predicate's flaw is not its strictness; it is that it asks a global
question about an advertised surface in order to answer a local question about
three specific calls.

---

## 5. The permit-lane hazard

**Verified.** `runtimes/brain-mux/src/wake.rs:205-233` maps `DispatchLane::Network`
— managed web *and* MCP — onto `PermitKind::ProviderStream`, the pool model
streams use, and multiplies by the tool's weight
(`units = base_units.saturating_mul(u64::from(weight))`). The pool is 48
(`compose.rs:46-63`), `base_units` is 1 (`compose.rs:158-163`), and a web tool's
weight is 4. **Twelve concurrent fetches exhaust the pool and defer every model
dispatch in the task.** The code comment already names the fix.

What this design does to it:

- **It fixes the underlying resource contention.** After the split, the permit no
  longer stands for "a socket, a TLS session, an HTML parse and up to 500 KB of
  buffer inside `brain-mux`". It stands for one outstanding request to an
  internal service. The thing the permit was rationing has left the process.
- **It inherits the collision unchanged** if nothing else is done, because the
  Brain still takes a `DispatchLane::Network` permit while it waits, and that
  lane still resolves to `ProviderStream`.
- **It would worsen it the moment §4 lands.** This is the sequencing finding and
  it matters more than either of the above: today twelve concurrent network-lane
  calls are *impossible* because `parallel` is false and the driver is serial.
  Batching plus concurrency is precisely the mechanism that produces them.
  **Do not enable concurrent batch execution before the network lane has its own
  permits.** Enabling batching alone (§4.3) is safe because the driver stays
  serial; enabling concurrency is not.

**The fix, and the resizing that should come with it.** Give the lane its own
`PermitKind`, sized independently in `Envelope::candidate_launch`. Then note that
the correct size changes character: it is now bounding *outstanding requests to
the executor*, not CPU inside `brain-mux`, so it should be considerably larger
than 12 and it should stop being the authoritative bound. **The authoritative
bound moves to the executor**, where it can be per-workspace and per-organization
instead of per-task, and where shedding load is a `429` rather than a silent
deferral. The Brain's permit becomes a coarse backstop against its own memory,
which is what a permit should be.

---

## 6. Does `network: none` still permit `web_search`?

### 6.1 The claim, split into its two halves

The claim put to the owner was: *`network: none` still permits `web_search`,
because the tool reaches the platform over the control channel the platform
already dials into the microVM.*

**The first half is true and structurally so.** `network: none` does not touch the
control path. `ingress_network_connectors: vec![ALL_INGRESS]` is attached
unconditionally, outside the `match` that zeroes egress
(`crates/aex-hands-control-aws/src/provider.rs:241-245`, **verified**). AWS's
proxy terminates TLS and re-originates plain HTTP inside the VM to
`0.0.0.0:8080`; the guest never initiates a connection, so a missing egress
connector breaks nothing inbound. A `none` session is fully controllable while
reaching nothing.

**The second half is false today, and it is the half the claim rests on.** There
is no path from a process inside the VM to the platform over that channel:

- The protocol is five verbs — `Start`, `Status`, `Cancel`, `Result`, `Attach`
  (`crates/aex-hands-agent/src/wire.rs:79-102`) — strictly request/response, and
  every one is initiated by the Brain.
- `Attach`, the only thing resembling server push, answers `501`:
  *"attached delivery is declared and not yet served; pull the result instead"*
  (`runtimes/hands-agent/src/serve.rs:246-258`), and the Brain hard-codes
  `DeliveryMode::Detached` on every start (`backend.rs:907`).
- The guest links no HTTP client and no TLS stack, held in place by a
  dependency-closure test on the shipped binary.
- There is no vsock, no unix socket, no host pipe and no shared file. One TCP
  port is the entire interface.

All **verified**. The mechanism the claim describes is the identity record's §3.1
proposal — a guest-local socket, a pending-slot queue and a `broker_reply` verb —
and none of it exists.

**Settled: the claim as stated must be withdrawn.** It describes a mechanism to
be built, not a path that exists, and it does not survive the framework record's
§8 shape at all, where the tool opens its own HTTPS connection and `network: none`
kills it outright.

### 6.2 What this design does instead

Under §1, `web_search` never runs in the sandbox, so the question dissolves rather
than being answered. It works under `network: none` because the sandbox is not on
its path, not because the control channel carries it.

And the resulting difference between the two tools is predictable rather than
arbitrary, on the rule in §1: `web_fetch` goes where the model pointed it, so it
uses the session's network and fails honestly when there is none — which is the
owner's decision 3, unchanged. `web_search` goes where the platform pointed it,
so it runs on the platform. A user can predict that from one sentence, and the
catalog already carries the discriminator as `EgressClass`.

Worth recording alongside: **no production code resolves the customer's requested
`NetworkMode` into a `NetworkPolicy`.** Every `NetworkPolicy` constructed anywhere
in the workspace is `::None` in a test fixture, there is no production
construction of `HandsGeneration` at all, and the API field is optional with no
documented default (`api/schemas/regional/sessions.yaml:122`). All **verified**.
`none` is currently a convention, not a resolved type — so this behaviour
difference is being designed before either behaviour is reachable.

### 6.3 If the owner insists `web_search` also be a sandbox executable

He may, for the composition claim. Then the broker is required and it should be
priced honestly rather than adopted quietly:

- A guest-local socket or a pending-slot queue on the existing port, which is
  loopback-reachable and locally unauthenticated (§3.3) — so the slot queue must
  treat local input as untrusted, which it can, because the slot carries no
  principal.
- A sixth verb, or `Attach` finally served — which needs a fix for the exclusive
  `guest.bound.write().await` held across the whole reap
  (framework §1.5i, **verified**).
- **The latency is the killer.** With `Attach` unserved, the Brain learns of a
  pending slot by polling `status`, and `poll_after` grows 250 ms → 4 s
  (`backend.rs:86-92`, **verified**). A brokered `web_search` could wait seconds
  before the platform even notices it was asked, inside a 15 000 ms tool budget.
- A blocking socket client inside a binary whose current virtue is that it links
  no client of anything.
- Identity-record D-7 — what happens to a guest process blocked on a broker slot
  across a suspend — is a state the operation machine does not describe.

**Recommendation: no.** The composition claim is satisfied more directly: the
same signed `rust-binary` is downloadable and runnable by the customer, who
supplies their own provider key through the documented environment variable. That
is composition, and it costs nothing.

One structural property falls out of that and is worth stating: **the tool reads
its credential from a named environment variable, and whether that variable is
set is a property of the host, not of the tool.** In the guest it is never set — the
`build_env` allowlist is deny-by-default and refuses anything unrecognised
(`crates/aex-hands-tools/src/command.rs:38-71`, **verified**) — so the tool
cannot work there even if someone routes it there by mistake. In the executor it
is set. The tool contains no code that knows where it is running.

---

## 7. Rate limiting, domain policy and spend caps

The prior records established, **verified**, that none of these exist. What
follows names an enforcement point for each rather than an intention, and marks
where it must coordinate with the billing record.

| Bound | Enforcement point | Status | Notes |
| --- | --- | --- | --- |
| Per-call admission | **executor**, step 7 of §3.2, before the vendor credential is touched | to build | must not trust the Brain's own check |
| Per-session spend | **Brain**, `Dimension::CostMicroUsd`, reserved before dispatch and reconciled after | built, never consumed | identity record §7.2 |
| Per-organization window | **executor**, conditional DynamoDB update | to build | identity record §7.3 |
| Domain policy for `web_fetch` | **nowhere, after the move** | regression | §7.2 below |
| Account pause | regional edge stage 8, already fail-closed | built, input never written | identity record D-5 |
| Metering | `ObservabilityMeter::ToolInvocations` counts; nothing prices | structurally unpriceable | coordinate with the billing record |

### 7.1 Two checks, not one, and why the second is the point

The Brain reserves against the session budget before dispatch; the executor
re-checks against the organization ceiling before spending. This looks redundant
and is not. **The executor's check is the only bound in the system that survives a
buggy or compromised Brain**, and it exists only because the executor is a
separate process with its own store. That is a security property the co-resident
design could not have had, and it is the best answer to "what does separation of
concerns actually buy" beyond tidiness.

Both should be capability-shaped rather than conventional: a `SpendPermit` that
the function which spends *requires as an argument*, in the manner of `Grant<C>`
in `crates/aex-regional-http/src/capability.rs` and of `DispatchTicket`'s
`FenceGuard`. A check a caller can forget is a check that will be forgotten.

### 7.2 Domain policy is a regression and should be recorded as one

`crates/aex-brain-managed-web/src/egress.rs` is the best-built thing in this area:
validation before DNS, one resolution, the whole answer screened, mixed
public/private rejected as a rebinding defence, the screened addresses pinned into
the client so there is no TOCTOU window, thirty denied IP ranges including IPv6
tunnel unwrapping, and every redirect hop re-validated rather than delegated to
`reqwest`. All **verified**.

**With `web_fetch` in the sandbox, none of it applies.** DNS is the customer's,
`/etc/hosts` is the customer's, and they can `LD_PRELOAD` the resolver. That does
not endanger the platform — a `public_internet` guest can already open a raw
socket to anything — but two things follow that the owner should hear:

1. **AEX stops being the originator of model-directed fetches**, which changes who
   receives the abuse report. It becomes the session's egress connector rather
   than `brain-mux`.
2. **There is no enforcement point left for a domain allowlist.** `NetworkPolicy`
   is a closed two-variant enum with no allowlist arm, no proxy arm and no
   per-domain arm. A workspace that wants "my agents may only reach these hosts"
   has nowhere to express it.

The options are: accept it as the price of the owner's decision 3; add a third
`NetworkPolicy` arm with a managed proxy, which is a much larger change; or keep
a Brain-side `web_fetch` for policy-required workspaces, which is two
implementations of one tool and should be refused. **Recommendation: accept it,
and record it as a known gap rather than discover it during an enterprise
review.** Decision **P-5**.

### 7.3 One warning the code already provides

The nearest existing thing to a per-organization counter is the OTLP quota, and it
is a cautionary tale: `services/regional-otlp/src/authority.rs:574-589` runs
`ADD reservedBytes …` with **no `ConditionExpression`**, so the numbers accumulate
forever and are compared to nothing, while `ErrorCode::TelemetryQuotaExceeded` is
declared on three routes and produced by none (**verified** in the identity
record). Likewise `telemetry.ingest_rate` sits in the limit registry with
published defaults and has no reader anywhere.

**Declaring a limit in this repository does not create one.** Adopt the identity
record's proposed `aex-workspace-check` rule — every `LimitId` whose dimensions
name a rate must have at least one non-test reference outside `aex-wire` — so this
design cannot quietly join that list. No such rule exists in
`tools/aex-workspace-check/src/rules.rs` today (**verified**).

The refusal itself needs no new vocabulary. `api/schemas/registries/errors.yaml`
already declares four `class: quota` codes — `slow_down`, `limit_exceeded`,
`rate_limited`, `telemetry_quota_exceeded` — of which only `rate_limited` has a
production emitter, and that one fires when DynamoDB throttles *us*
(`crates/aex-regional-http/src/projection.rs:144`), not when we throttle a
workspace. All **verified**. A ceiling refusal should reuse `limit_exceeded` and
say plainly which ceiling was reached, with the remaining budget deliberately
absent so a hostile customer cannot binary-search it.

### 7.4 Coordination with the billing record

Two points the billing record must own and this one must not duplicate:

- **Platform-paid tool spend needs a bound distinct from customer credit**, and
  this record agrees: the customer's prepaid balance is the wrong instrument,
  because the platform's exposure is a cost of goods, not a receivable. The
  per-organization window in the table above is that bound, and it belongs at the
  executor rather than beside the credit ledger.
- **Pricing a platform-paid call means a fifth `Meter` or breaking the
  `Meter` / `ObservabilityMeter` disjointness the code was written to guarantee**
  (**verified**: `ObservabilityMeter` is structurally barred from the rate card
  and the outbox refuses to publish one as `Unrepresentable`). Until that is
  decided, a platform-paid `web_search` is unbilled by construction — and would
  be unbilled anyway while `BillingMode::Shadow` is the default. That is the
  billing record's decision, not this one's; this record only asserts that the
  *ceiling* must exist before the tool is advertised, whether or not the *price*
  does.

---

## 8. What must be built, in order

The ordering is the deliverable. Several of these are safe only after an earlier
one.

1. ~~**Confirm §4.2 against the gating suite.**~~ **Done 2026-08-10, and the
   answer was the bad one.** A tool-bearing request is *not* reachable from the
   gating suite: the one live test that mentions tools hardcodes an empty tool
   list and `parallel_tools: false`, so DeepSeek returns through its no-tools
   path and never reaches the refusal. The gate was green and the defect was
   invisible to it. Production was not green by the same reasoning —
   `brain-mux` composes `managed_web`, so `web_fetch` is advertised on every
   deployed task and every tool-bearing turn on four dialects would have failed
   at request build.
2. ~~**Split `parallel` from `parallel_safe`** (§4.3).~~ **Done 2026-08-10.**
   `ToolAdvertisement::allows_parallel_emission` is now the sole provider-facing
   value and consults the model's declared capability alone. Note that
   `parallel_safe` consequently has **zero production readers** until step 8 —
   it is written, tested and documented, and nothing consumes it yet.
3. ~~**Give the network lane its own `PermitKind`** (§5).~~ **Done 2026-08-10.**
   `PermitKind::NetworkLane`, sized at 128 — the safety cap of 32 activations
   times the heaviest declared tool weight of 4, i.e. one outstanding network
   call per admitted activation, which is all a serial driver can reach. No
   concurrency enabled.
4. **Stand up the executor** as one crate with one handler, deployed as
   `rust-lambda` first (§2.3). Includes the `ToolExecRequest` type with no
   principal field and its closed-field test.
5. **Add the `ToolExec` assertion audience and verify it at the executor**
   (§3.2), with the `brain-mux` signing key as a new `kid`.
6. **Build the two spend bounds** (§7): session-side reserve/reconcile on
   `CostMicroUsd`, executor-side conditional update per organization.
7. **Then, and only then, per-batch concurrent execution** (§4.4).
8. **Independently: the framework's seven gaps** for sandbox-resident tools
   (its §11). They do not block 1–7 and 1–7 do not block them. Of those, inline
   `Exec.stdin` bytes and the guest wall clock are the two that gate any tool
   running in the sandbox at all.

Steps 1–3 are worth doing whatever the owner decides about placement.

---

## 9. What this design does not fix

Stated so it is not discovered later as a surprise.

- **The Brain remains the universal tenant-crossing component.** Its compromise is
  still total. §2.4.
- **Nothing writes `finance.billing_account.state`.** The pause gate, its view,
  its fail-closed absence handling, its regional projection and its lease
  termination are all built and correct; the column is inserted as `'active'` and
  never updated by any production path. A customer whose credit reaches zero keeps
  working. That is the largest open defect touching this area and it is not this
  record's to close.
- **`web_fetch` loses SSRF screening** as a consequence of a decision already
  taken. §7.2.
- **B2 is unearned.** Every live boundary probe panics because nothing is
  deployed. This design is written so no guarantee depends on it, which is why
  §3.2 has a cryptographic step and not only a network one.
- **None of this is reachable in production yet.** `TransactionIntent::CreateSession`
  is refused at `crates/aex-session-app/src/plan.rs:661-664`, `create_generation`
  has only test callers, and `infra/` deploys neither `brain-mux` nor
  `runtime-control-worker` (**verified** in the identity record). That makes every
  choice here cheap to adopt now and expensive to retrofit later, which is the
  argument for deciding rather than deferring.

---

## 10. Decisions for the owner

**P-1 — Lambda now, Fargate later, or Fargate now?**
The identity design is identical either way (§2.1). `rust-lambda` costs nothing
at idle for a surface advertised to nobody; `rust-oci-service` costs ~$72/month at
two tasks plus an internal-ALB module that does not exist, and buys single-digit
milliseconds on a call the model is blocked on. *This record recommends the same
crate shipped as a Lambda first.* The owner has said ECS; this is the one place
this record asks him to reconsider, and only about timing.

**P-2 — Which `PrincipalKind` does a Brain-minted assertion declare?**
The envelope's principal fields are shaped for a customer presenting a credential.
An activation-minted assertion has no presented credential. Either add a principal
kind, or define the tool-exec audience as one where the principal slot names the
session. Unresolved here because inventing it silently is how wire layouts rot.

**P-3 — Does the 32-byte `credential_binding` slot carry the canonical argument
digest for the tool-exec audience?**
It binds the request body to the assertion, which matters only against an attacker
who can reach the executor's listener with a stolen envelope — which §3.3 argues
nobody can. Cheap defence in depth; also a per-audience reinterpretation of a
field, which is the kind of trick that confuses a later reader. Recommend not
doing it, and instead pinning the invariant that the executor never has a public
listener as a Terraform-level assertion.

**P-4 — Do platform tool calls become `DurableDetached` effects?**
Parking the activation is what actually removes the wait from `brain-mux` (§2.4).
It costs the executor a durable operation store, a result-query path and recovery.
Not required on day one; the design should not foreclose it.

**P-5 — Is losing domain policy on `web_fetch` accepted?** §7.2.

**P-6 — What are the numbers?**
Per-organization calls per minute and per day, the session `CostMicroUsd` grant,
the new lane's permit count, and the executor's own concurrency. Every one is a
guess today. They belong in `api/schemas/registries/limit-defaults.v1.json` so
they are versioned, and §7.3's check rule should be added in the same change so
they are read.

---

## 11. Verified versus inferred

**Verified by reading code or committed configuration:** `EgressClass`'s four arms
and `web_fetch`/`web_search`'s declared classes, weights, timeouts and bounds; the
`parallel_safe` predicate, the `parallel` computation, its participation in the
canonical request digest, and the four provider dialects that refuse a
tool-bearing request when it is false; that `Determinism` is derived from
`EffectClass`; that the activation driver executes exactly one tool call per step
in emission order with no concurrency primitive anywhere in the Brain crates;
`DispatchLane`'s three arms, the `Network` → `ProviderStream` mapping, the pool of
48, the base unit of 1 and the weight multiplication; the guest protocol's five
verbs, its strictly inbound request/response shape, `Attach` answering `501`, and
the hard-coded `DeliveryMode::Detached`; `poll_after` growing 250 ms → 4 s; that
ingress connectors are attached unconditionally and only egress varies with
`NetworkPolicy`; that no production code constructs a `HandsGeneration` or
resolves `NetworkMode`; that the guest's own listener is loopback-reachable and
locally unauthenticated; the guest's dependency-closure bans on TLS stacks and
cloud SDKs; the deny-by-default child-process environment allowlist; the
`egress.rs` policy in full and that it is `const` and not per-workspace
configurable; the assertion envelope's layout, its 30-second clamp, its
verification order, its five-of-eight audience bits, the `AssertionSigner` /
`LocalSigner` interface and the recorded rejection of KMS asymmetric signing; that
`central-authz` still holds `reserved_concurrency = 200` while the regional plane
declares it has no client; that `alb-public` hardcodes `internal = false`, that
`ecs-service` requires a target group, and that `region-application` instantiates
no `lambda-function` module; that `brain-mux` is pinned to 2 vCPU / 4 GiB, two
production tasks, no autoscaling and `AEX_MAX_ACTIVE_ACTIVATIONS = 16`; that only
three `rust-oci-service` units exist at all; the four size-only fields of
`EffectiveLimits` and the absence of any rate field; that `telemetry.ingest_rate`
has no reader; that the OTLP quota counter carries no `ConditionExpression`; the
four `class: quota` error codes and their single misdirected emitter; the four
priced meters, the three structurally unpriceable observability counters and their
test-locked disjointness; and that `aex-workspace-check` carries no
limit-reference rule.

**Adopted from the prior records rather than re-verified here:** the finance and
metering trace (identity record §1.8, §1.9), the budget dimensions, the
account-state chain and its unwritten input, the OTLP quota's missing
`ConditionExpression`, the `telemetry.ingest_rate` gap, the framework record's
nine defects in its §1.5, and the image-build and release-graph accounts. Each is
cited to a line in its own record.

**Inferred, and flagged:** every latency figure in §2.2 — the in-VPC round trip,
the Lambda warm and cold invoke costs, and the vendor handshake — none measured;
that a private-subnet service is unreachable from a microVM, which follows from
B2 and is *declared rather than earned*, hence the cryptographic step in §3.2;
that appending a sixth `AssertionAudience` is wire-safe, which follows from
declaration order being wire order and from three free bits, but was not exercised;
and that a batch consuming one step per call can exhaust `max_steps_per_turn`,
which follows from the driver's structure but has no test.
