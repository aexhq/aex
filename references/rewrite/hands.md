---
title: Hands stream handoff — protocol semantics, guest agent and tools, trusted control, lifecycle and metering
description: What the Hands stream implemented on rw/hands, what it deliberately left as tracked unavailable evidence, every cross-stream type it publishes with its exact path, every change it needs from a peer, and every decision it took beyond the orchestrator conventions.
keywords:
  - hands
  - microvm
  - true idle
  - guest agent
  - lifecycle
  - metering
audience: implementation agents and maintainers
status: accepted
last_verified: 2026-08-02
related:
  - references/rewrite/contracts.md
  - references/rewrite/test-architecture.md
  - references/rewrite/usage.md
  - references/rewrite/regional-stores.md
---

# Hands stream handoff

Plans of record: `references/rust-native-rewrite-2026-07-31/plans/10-hands-runtime.md` and `references/rust-native-rewrite-2026-07-31/plans/07-brain-core.md`, in the parent workspace.

Branch `rw/hands`. Everything below is on that branch and nothing is pushed. This
stream resumed an interrupted predecessor whose work was preserved as a
`wip(hands)` commit; §6 records what was kept and what was discarded.

## 0. Continuation checkpoint — 2026-08-02

This checkpoint supersedes older statements below that the Rust provider SDK,
runtime adapters, regional port, or usage ingresses are absent. The continuation
is unpushed and undeployed; it read no credential and called no AWS API.

| Commit | Landed state |
| --- | --- |
| `9a9bcfd9` | The official `aws-sdk-lambdamicrovms` adapter, documented managed connector ARNs, exact request IDs and closed provider-state mapping. |
| `0d3a00fe` | `RuntimeActivityDynamoStore` implements the canonical `aex_runtime_control::store::RuntimeActivityStore` port. |
| `7d353923` | `OpenHandsEffectCounter` performs the bounded strongly consistent session-authority recount. |
| `eccbba5d` | Production composition, canonical usage drafts and category ingress, atomic lifecycle accounting, and the durable transactional usage outbox. |

`runtime-control-worker` now resolves all five production ports, uses the SDK's
regional endpoint by default, and probes both tables, all three queues and
`ListMicrovms` before Lambda polling begins. A lifecycle settlement commits the
durable intent update, immutable receipt, final generation state, accounting
cursors and up to three usage drafts in one `DynamoDB` transaction. Queue
delivery happens afterward; a lost response or usage-ingress outage therefore
redrives the outbox without repeating the provider effect. The compute and
storage workers accept the strict `usage_fact_draft.v1` envelope and admit each
SQS record independently through the category authority's `RecordFact` use case.

Focused continuation gates:

```text
cargo test (eight affected production/domain packages)
  351 passed; 0 failed; 0 ignored

cargo test -p aex-regional-test-support
  46 passed; 0 failed

cargo clippy (eight affected packages) --all-targets -- -D warnings
  clean

cargo run -p aex-workspace-check
  134 members and 141 packages satisfy every structural and registry rule
```

One older crash-matrix gap remains explicit: taking the transitional generation
fence and recording the provider intent are still separate store calls, and the
worker reports an existing `dispatched`/`unknown` intent as reconciling without
yet executing the pure `ReconcileStep` plan. A crash in either window cannot
admit work or blindly repeat an effect, but it can leave a generation awaiting
operator reconciliation. Do not claim the runtime lifecycle production-ready
until active reconciliation closes that gap and the declared live `MicroVM`
cases pass.

## 1. What is implemented

### `crates/aex-runtime-control` — the pure lifecycle and true-idle model

No clock, no `tokio`, no AWS client, no I/O. That is what makes the 180000 ms
boundary an exhaustive table rather than a timing observation.

| Module | What it owns |
| --- | --- |
| `clock` | saturating millisecond arithmetic over the wire `Timestamp` |
| `shape` | the five compute shapes as a `ShapeCapacity` extension over `aex_wire::types::ComputeSize`, with the golden capacity table and the `$0.155/hour` derivation |
| `generation` | the immutable generation tuple, the eleven-state machine, the F1–F6 fence algebra, admission and settlement |
| `idle` | `IdleAssessment`, the exact 179999/180000 ms boundary, keepalive issuance and the jittered evaluation schedule |
| `lifecycle` | provider state mapping with no unknown-state fallback, `ProviderCall` classification, the actively enforced eight-hour lifetime, intent records and reconciliation |
| `pressure` | the total-order pressure ranking and the high/low-water release plan |
| `usage` | canonical `FactDraft` derivation from provider evidence; compute/memory reservation, snapshot residence rounding, `SnapshotIo` as zero-dollar observability, and the `UsageFactSink` port |
| `store` | the `RuntimeActivityStore` port the regional-stores peer implements |

### `crates/aex-hands-agent` — the frame codec and the guest supervisor

| Module | What it owns |
| --- | --- |
| `wire` | the 40/56-byte preambles, the five verbs, the pinned nine-step hostile decode order, result-payload splitting and body verification |
| `crc` | CRC-32C, checked against the published Castagnoli vectors |
| `journal` | the on-disk operation journal, its three ordering rules and replay classification |
| `capture` | output retention bounds, UTF-8-safe attached framing and the mirror cap |
| `session` | the start identity table, `status`/`result`/`cancel`, the cancel ladder and the provider lifecycle hooks |

### `crates/aex-hands-tools` — the guest executors

`port` (`GuestFs`/`GuestProc` and their typed errors), `command` (argv bounds and
the deny-by-default environment), `filesystem` (read, list, stat, write, the
revision-checked edit and its four matching tiers), `observation` (search and the
path-containment properties).

### `crates/aex-brain-hands` — the Brain-side adapter

`adapter` (the three materialization collapse layers, admission and settlement
plans, transport-mode selection, `HandsError` routing) and `operation` (the
`git`/`package_install`/`code_run` argv constructors and the resumable
`ResultAssembly`). The frame codec is re-exported from `aex-hands-agent`, not
reimplemented.

### `crates/aex-hands-control-aws` — the trusted provider adapter

`provider` (the `MicrovmControlApi` seam, `RunRequest`, `RunHookPayload`,
`EndpointToken`, the IAM action sets), `aws` (the official generated
`aws-sdk-lambdamicrovms` binding), and `lifecycle` (provider answer
classification and the deterministic transition awaits).

### `crates/aex-runtime-control-aws` — the worker's composition

`composition` (the suspend transition and the authoritative recount), `queue`
(partial-batch folding and poison quarantine), `usage_ingress` (strict
category-scoped SQS drafts), and `worker` (the complete lifecycle engine,
transactional usage outbox and scheduled due scan).

### Deployables

`workers/runtime-control-worker` binds the runtime-activity store, exact session
recount, official provider client, and compute/storage ingresses. Its `health`
module owns `/internal/healthz` and `/internal/readyz`, named readiness
dependencies, the two work domains and the two usage categories it may write to.
`runtimes/hands-image` gains an `image` module with the build inputs, the package
manifest, the NEVRA lockfile comparison, the eight per-region variants and the
rootfs contract. `runtimes/hands-agent` keeps its validated composition root.

### Live companions

`tests/live/aex-live-hands-image/tests/boundary.rs`,
`tests/live/aex-live-runtime-control-worker/tests/lifecycle.rs` and
`tests/live/aex-live-hands-agent/tests/guest.rs` each declare their cases and
**fail loudly** when the live lane selects them. None self-skips. Each file states
which unit-level test already covers the off-VM half of the same control, so a
reader can see exactly what is and is not earned.

## 2. Gate output

Run at the end of the stream, from the worktree root, with `CARGO_BUILD_JOBS=4`.

```
cargo fmt --all
  (clean)

cargo clippy -p aex-brain-hands -p aex-hands-control-aws -p aex-hands-agent \
  -p aex-hands-tools -p aex-runtime-control -p aex-runtime-control-aws \
  -p runtime-control-worker -p hands-agent -p hands-image --all-targets -- -D warnings
  Finished `dev` profile [unoptimized + debuginfo] target(s)

cargo nextest run -p aex-brain-hands -p aex-hands-control-aws -p aex-hands-agent \
  -p aex-hands-tools -p aex-runtime-control -p aex-runtime-control-aws \
  -p runtime-control-worker -p hands-agent -p hands-image
  Summary: 246 tests run: 246 passed, 0 skipped

cargo check --workspace --all-targets
  Finished `dev` profile [unoptimized + debuginfo] target(s)

cargo run -p aex-workspace-check
  aex-workspace-check: 133 member(s) and 139 package(s) satisfy every structural
                       and registry rule
  aex-workspace-check: 573 unearned-evidence row(s) recorded in the
                       source-rewrite phase

cargo run -p aex-workspace-check -- registry build
  aex-workspace-check: wrote release/test-registry.json and
                       release/unearned-evidence.json
```

Only `aex-hands-agent` declares `[package.metadata.aex.targets]`, because it is
the only owned crate with separate `[[test]]` targets (`hostile_frames`,
`no_cloud_authority`, `no_guest_billing`). Every other owned crate keeps its
evidence in inline unit modules, which the declared `unit` layer already
collects, and says so in `not_applicable.targets` rather than naming a target
that does not exist.

Zero `#[ignore]`, zero environment-conditional self-skips, zero retries. The
default nextest profile already excludes `aex-live-*` by `default-filter`, so the
live companions are declared and unearned rather than silently green.

## 3. Types published to peers

| Consumer | Path | What |
| --- | --- | --- |
| regional stores | `aex_runtime_control::store` | `RuntimeActivityStore`, `GenerationPointer`, `GenerationPlan`, `GenerationCommit`, `LifecycleIntentPlan`, `LifecycleReceiptPlan`, `LifecycleReceipt`, `IdleProbe`, `RuntimeShard`, `PageBudget`, `RuntimeDuePage`, `RuntimeStoreError` |
| regional stores, brain | `aex_runtime_control::generation` | `HandsGeneration`, `GenerationHead`, `GenerationState`, `Revision`, `TransportMode`, `ImagePin`, `ImageCapability`, `NetworkPolicy`, `LimitsRevision`, the F1/F4/F6 functions |
| brain | `aex_brain_hands::adapter` | `MaterializeStep`, `AdmitPlan`, `SettlePlan`, `HandsError`, `Alpn`, `transport_mode`, `pool_size`, `max_in_flight` |
| brain | `aex_brain_hands::operation` | `ResultAssembly`, `IncorporateError`, `ConstructedCommand`, `CodeLanguage`, `PackageManager`, `git`, `package_install`, `code_run` |
| brain, guest | `aex_hands_agent::wire` | `RequestPreamble`, `ResponsePreamble`, `Frame`, `FrameExpectation`, `FrameError`, `Verb`, `encode_request`/`decode_request`, `encode_response`/`decode_response`, `split_result_payload`, `verify_body`, `DECODE_STEPS` |
| usage | `aex_runtime_control::usage` and `aex_usage_domain::ingress` | `UsageFactSink`, `FactDraftEnvelope`, `UsageCategory`, `FactContext`, `HandsUsage`, `SnapshotResidence`, `SnapshotIo`, `derive_usage`, `derive_facts`, `category_of` |
| delivery | `hands_image::image` | `ImageLock`, `PinnedPackage`, `LockVerdict`, `ImageVariant`, `PackageGroup`, `ROOTFS_CONTRACT`, `variants()` |
| all deployables | `aex_hands_control_aws::provider` | `MicrovmControlApi`, `RunRequest`, `RunHookPayload`, `EndpointToken`, `RUNTIME_IAM_ACTIONS`, `FORBIDDEN_IAM_ACTIONS` |

## 4. Changes needed from peers

### Contracts (`aex-hands-protocol`)

Consumed as landed; not edited by this stream. Four gaps remain, each currently
worked around inside this stream's own crates:

1. **`OperationRequest` has no `Browser` arm.** The capability gate exists and is
   evaluated before anything spawns (`aex_hands_agent::session::requires_browser`),
   but it can never fire until the arm exists. The match is exhaustive, so adding
   the arm will not compile until someone decides which side of the gate it is on.
2. **`OperationRequest::ProcessStatus` has no `from_offset`/`max_bytes`.** Reading
   a background process's incremental output can only return a tail today, which
   loses backlog. The guest-side paging already exists on `Journal::read_output`.
3. **`Materialize`/`Persist` carry a bare `ContentHash`, not a presigned plan
   URL.** The guest holds no AWS credential and cannot resolve a hash, so these two
   arms are currently unimplementable end to end.
4. **`OperationFailure.reason` is a free `String`.** This stream emits the stable
   codes from plan 10 §3.7 (`capability_unavailable`, `stale_write`,
   `guest_interrupted`, …) but nothing enforces the closed set.

### Orchestrator conventions §4

`rust-toolchain.toml` needs `aarch64-unknown-linux-musl` in its target list. The
guest binary must be static so an ordinary customer `pip`, `dnf` or `ldconfig`
cannot break the supervisor out from under its own operation.

### Regional stores (plan 05)

- The `runtime-activity` `HEAD` needs `transportMode`, `protocolVersion`,
  `agentBuild`, `capabilities` and `imageArtifactDigest`; the plan 05 attribute
  list has the first four missing.
- Plan 05 sketches the returned intent record as `LifecycleIntent`. That name is
  taken by the contract's own transported action union, so the record published
  here is `aex_runtime_control::lifecycle::IntentRecord`. **No alias is published**:
  an alias would make a glob import resolve to the wrong type, which is precisely
  the hazard the rename exists to avoid.

### Brain (plan 07)

`HandsPort` does not exist yet — `aex-brain-domain` and `aex-brain-application`
are still skeletons. `aex_brain_hands::adapter::HandsError` already distinguishes
`GenerationLost`, `Fenced`, `Interrupted`, `CapacityQueued`,
`CapabilityUnavailable` and `Transport`, with `retry_same_effect()` and
`interrupts()` as the routing predicates the recovery matrix needs.

### Usage + finance

`derive_facts` now produces canonical `aex_usage_domain::fact::FactDraft` values.
Equal provider evidence produces equal authority identities and intent hashes;
the category worker alone assigns admitted sequence and time. Compute and memory
use provider-shape evidence with `FactBasis::Reserved`; storage floors interior
snapshot closes and ceils the terminal tail. The strict
`usage_fact_draft.v1` envelope redundantly checks queue, envelope, worker and
draft category. `runtime-control-worker` remains bound only to compute and
storage; its health tests prove there is no transfer binding.

## 5. Boundary controls and the test that falsifies each

| # | Control | Falsifying test | Status |
| --- | --- | --- | --- |
| B1 | No AWS execution role | `aex-hands-control-aws` `provider::tests::no_execution_role_can_be_threaded_through_a_launch` — the struct has no such field and `deny_unknown_fields` refuses one from outside | earned |
| B1 | IMDS unreachable from the guest | `aex-live-hands-image` `b1_the_guest_reaches_no_instance_credential` | unearned, declared |
| B2 | No AEX VPC egress or private route | `aex-hands-control-aws` `provider::tests::the_launch_request_shape_is_exact` — egress is exactly `{INTERNET_EGRESS}` or empty | earned |
| B2 | Private endpoint unreachable | `aex-live-hands-image` `b2_the_guest_reaches_no_private_aex_route` | unearned, declared |
| B3 | No shell ingress | `aex-hands-control-aws` `provider::tests::no_shell_ingress_action_is_in_the_runtime_role` — ingress uses the documented `ALL_INGRESS` managed connector and no `*Shell*` action is in the runtime set | earned |
| B4 | No managed secret in the guest | `aex-hands-control-aws` `provider::tests::the_run_hook_payload_key_set_is_closed_and_sorted` — closed sorted key set; the launch ticket has no way back in | earned |
| B4 | No secret in the environment | `aex-hands-tools` `command::tests::the_environment_is_deny_by_default` and `no_aws_variable_ever_reaches_a_spawn_environment` | earned |
| B4 | No canary in a real rootfs | `aex-live-hands-image` `b4_no_planted_canary_secret_appears_anywhere_in_the_guest` | unearned, declared |
| B5 | Endpoint token never leaves trusted memory | `aex-hands-control-aws` `provider::tests::an_endpoint_token_never_renders_its_secret` — no `Display`, no `Serialize`, redacted `Debug` | earned |
| B5 | Proxy strips the header | `aex-live-hands-image` `b5_the_guest_never_observes_the_endpoint_auth_header` | unearned, declared |
| B6 | Guest binary has no cloud authority | `aex-hands-agent` `no_cloud_authority.rs` — scans the whole normal-and-build dependency closure from `cargo metadata`, with a self-check proving the matcher works | earned |
| B6 | Static binary, no SDK in the image | `aex-live-hands-image` `b6_the_agent_binary_is_static_and_carries_no_sdk` | unearned, declared |
| B7 | Whole-VM ceilings | `aex-runtime-control` `shape::tests::the_golden_shape_table_is_exact`, `lifecycle::tests::the_lifetime_margins_are_exact`, `generation::tests::admission_stops_at_the_shape_concurrency_ceiling` | earned |
| B7 | Hostile root workloads contained | `aex-live-hands-image` `b7_hostile_root_workloads_are_contained_by_the_vm` | unearned, declared |
| B8 | No guest-reported fact is billable | `aex-hands-agent` `no_guest_billing.rs` — a forged length and a forged digest both journal nothing, and the manifest scan proves the guest links no crate that could name a `UsageFact` | earned |
| B8 | Brain refuses a forged terminal | `aex-brain-hands` `operation::tests::a_deliberate_length_or_digest_mismatch_journals_nothing_and_keeps_diagnostics` and `brains_own_body_ceiling_bounds_a_guest_that_declares_a_huge_body` | earned |
| B9 | Cross-tenant isolation is the `MicroVM` boundary | `aex-live-hands-image` `b9_two_generations_cannot_see_each_other` — there is no unit-level half, because the control **is** the hypervisor | unearned, declared |

Deleted rather than ported, with the reason stated in code:
`setpriv` per-uid isolation, the `nft` IMDS firewall and the `nft` egress counter.
`runtimes/hands-image` `FORBIDDEN_INSTALL_PACKAGES` and `FORBIDDEN_ROOTFS_PATHS`
are the regression guards, asserted by
`the_deleted_packages_are_absent_from_every_group` and
`the_deleted_artefacts_have_no_path_in_the_contract`.

## 6. The tool surface actually implemented

| Tool | Where | State |
| --- | --- | --- |
| `fs_read` | `aex_hands_tools::filesystem::read_file` | line windows, byte cap, explicit truncation notice, whole-file digest |
| `fs_list` | `filesystem::list_dir` | `lstat` only, symlinks never followed, `.git` listed never descended, entry cap |
| `fs_stat` | `filesystem::stat_path` | `lstat`, link target reported not followed |
| `fs_write` | `filesystem::write_file` | atomic via the port's temp-sibling rename |
| `fs_edit` | `filesystem::edit_file` | stateless revision check, four matching tiers reported, `stale_write` with the current digest, length and window |
| `fs_search` | `observation::search` | literal, case-insensitive and glob matching; deadline, cap, binary skip and size skip all as explicit notices |
| `shell_exec` | `command::build_spawn` + `build_env` | argv bounds, no shell, deny-by-default environment |
| `git` | `aex_brain_hands::operation::git` | Brain-side argv constructor, fifteen `GIT_*` variables stripped |
| `package_install` | `operation::package_install` | four managers, 64-package ceiling |
| `code_run` | `operation::code_run` | three languages, file-not-stdin, wall-bound ceiling |
| `process_status` / `process_stop` | `aex_hands_agent::session` + `journal::read_output` | paging exists; the wire arm lacks `from_offset` (see §4) |
| `browser_*` | gate only | `requires_browser` is evaluated before any spawn; the executor waits on the contract arm |
| `Materialize` / `Persist` | not implemented | blocked on the presigned-plan contract change (§4) |

`SearchPattern::Regex` is matched as a literal for now. That is **narrower** than
the contract promises, never wider, so no caller receives a match it should not
have; a bounded engine is a later, auditable addition.

## 7. What was kept and what was discarded from the interrupted predecessor

Kept, ported onto the landed contract types: the shape capacity table and its
golden test, the true-idle boundary table and its properties, the generation state
machine and fence algebra, the provider state mapping, the lifetime margins, the
intent/reconciliation model and the pressure ranking.

Discarded: `wire_pending.rs` in full, and the local redefinitions of `Meter`,
`UsageFact`, `FactId`, `Attribution`, `ServiceTime`, `FactBasis`, `RuntimeReceipt`,
`TrueIdleEvidence`, `KeepaliveLease`, `ComputeSize`, `SchemaVersion`, `Timestamp`
and every id newtype. All of those now exist in `aex-wire`,
`aex-internal-contracts` or `aex-hands-protocol`, and keeping a second copy would
have made the `RuntimeActivityStore` port unusable by the regional-stores peer that
binds to it.

## 8. Decisions taken beyond the orchestrator conventions

| ID | Decision | Rationale |
| --- | --- | --- |
| HS-01 | The binary frame codec lives in `aex_hands_agent::wire`, not in `aex-hands-protocol` | The contracts stream owns and generates the protocol crate and this stream does not edit it. The guest is the side that must survive hostile bytes, so the codec sits beside the guest supervisor; `aex-brain-hands` re-exports it, so there is one codec and not two that can disagree. The alternative — a new `aex-hands-wire` crate — would have deviated from the frozen member inventory for no gain. |
| HS-02 | `aex_runtime_control::shape` is an extension trait over `aex_wire::types::ComputeSize`, not a second enum | One shape vocabulary workspace-wide and no mapping table to drift. |
| HS-03 | The 180000 ms decision lives in `IdleAssessment`, which pairs the contract's `TrueIdleEvidence` with a head-held `last_busy_at` | The wire evidence carries counters only. Keeping `last_busy_at` off the wire keeps a value a guest might try to influence out of the transported snapshot. |
| HS-04 | The durable lifecycle record is `IntentRecord`, and no `LifecycleIntent` alias is published | `aex_hands_protocol::lifecycle::LifecycleIntent` already names the transported action union. An alias would let a glob import silently resolve to the wrong type. |
| HS-05 | Snapshot I/O is `SnapshotIo`, a type with no `Meter` and no conversion into a `UsageFact` | OD-25 says snapshot I/O is zero-dollar observability. Making it structurally unable to become a fact is stronger than a comment saying it must not. |
| HS-06 | "No allocation before the payload bound" is proven by asserting the decoded payload is a subslice of the caller's buffer, not by counting allocations | A `#[global_allocator]` needs `unsafe impl`, which the workspace forbids. The pointer-containment assertion is strictly stronger: a decoder that copied, or that reserved from `payload_len` before checking it, could not return a pointer inside the input. |
| HS-07 | Directory fsync is a compile-time capability, `journal::DIRECTORY_SYNC_AVAILABLE` | The guest target is POSIX and always takes the real path. On a non-POSIX development host the directory half of ordering rule 1 cannot be exercised, and the constant says so rather than letting a green run imply coverage it did not have. |
| HS-08 | `SearchPattern::Regex` is matched as a literal until a bounded engine lands | Narrower than promised, never wider. A caller can only miss a match it should have had, never receive one it should not. |
| HS-09 | Settlement is deliberately **not** conditional on the fence | A suspend advances the fence while an operation is open. A fence-conditional settle would strand the counter above zero and the generation would never become idle. |
| HS-10 | `runtime-control-worker` holds no transfer-authority binding at all | Hands egress is not charged at launch (OD-26) and snapshot I/O is not a transfer fact (OD-25), so there is nothing for the binding to do. Not holding it is stronger than holding it and not using it. |

## 9. Known gaps

Every gap below is recorded rather than guessed at. Each is either a live probe
this run cannot make (OD-07) or a peer change this stream cannot land.

| Gap | Handling |
| --- | --- |
| Live provider compatibility | The official `aws-sdk-lambdamicrovms` binding and documented `ALL_INGRESS`/`INTERNET_EGRESS` connector ARNs are pinned and unit-tested. A real `RunMicrovm`/suspend/resume/terminate canary remains unearned until the live lane runs. |
| Accepted range of `idlePolicy` fields | `maxIdleDurationSeconds` minimum is 60; the maximum is undocumented. Setting 28800 must be confirmed or the largest accepted value pinned. |
| `clientToken` length and charset | `aexgen-{generation}` is 37 characters with the wire id spelling, asserted by test. The API's constraint is undocumented. |
| Run-hook payload length metric | The 4096-UTF-8-byte hold is asserted before dispatch. Whether the service counts scalars, UTF-16 units or bytes needs the documented ASCII and four-byte-scalar canary. |
| Whether the endpoint counts h2 streams or TCP connections | Decides whether `Multiplexed` actually removes the 8/16/32/64/128 ceiling. `max_concurrent_operations` stays at `min(32, max_connections * 2)` until measured. Declared as `aex-live-hands-agent` `the_endpoint_connection_cap_counts_streams_or_connections`. |
| Provider-authoritative per-generation transmit bytes | `RuntimeReceipt.transmit_bytes` is `Option`; `None` produces no transfer fact at all, asserted by `usage::tests::no_transfer_fact_exists_while_the_provider_exposes_no_transmit_receipt`. |
| Provider-reported snapshot size | `SnapshotResidence.bytes` is a field, not a constant, so a provider-reported size becomes authoritative without a type change. |
| Cross-compilation to `aarch64-unknown-linux-musl` | **Unearned.** The build host is Windows and the target is not installed. The guest code is portable and tested natively; the cross-build and the resulting static-binary assertions are declared in `aex-live-hands-image` (`b6_the_agent_binary_is_static_and_carries_no_sdk`) rather than claimed. |
| Local `hands-image` boot harness | **Unearned.** Running the rootfs as an `aarch64` container needs a container runtime and QEMU user emulation, neither of which is available here. The image *definition*, package manifest, lockfile comparison and rootfs contract are all implemented and unit-tested; the boot itself is declared in the live companion. |
| Browser capability qualification | The gate is implemented and fails closed. Chromium under an ARM64 `MicroVM` at a 2 GiB baseline is unproven, so the capability ships disabled. |
| Keepalive pricing | `KEEPALIVE_MAX_MS` defaults to 3600000. The commercial values are private inputs (OD-09) and are not invented here. |

## 10. What this stream did not do

Nothing was deployed, published or credentialed. No `MicroVM` was launched, no
image was pushed, no AWS API was called, and no `.env` was read. There is no shim,
no launch-authority Lambda, no opaque launch ticket and no compatibility layer for
the retired WebSocket protocol.

---

## 11. Composition

Branch `rw/deploy-hands`, off `main`, unpushed. This wave replaces the three
typed `NotImplemented` roots with real ones and makes the guest image buildable.

### 11.1 `workers/runtime-control-worker`

Configuration is total and typed. Twelve variables are required and each is
named when it is absent; the region resolves through `Region::from_name`, every
queue must be regional HTTPS, and the compute and storage ingresses may not be
the same queue. `AEX_MICROVM_CONTROL_ENDPOINT` is an optional explicit test or
compatibility override; production uses the official SDK's regional endpoint.

The twelve: `AEX_PLANE`, `AEX_REGION`, `AEX_RUNTIME_ACTIVITY_TABLE`,
`AEX_SESSION_AUTHORITY_TABLE`, `AEX_RUNTIME_LIFECYCLE_QUEUE_URL`,
`AEX_USAGE_COMPUTE_QUEUE_URL`, `AEX_USAGE_STORAGE_QUEUE_URL`,
`AEX_HANDS_IMAGE_IDENTIFIER`, `AEX_RUNTIME_DUE_SHARDS`, `AEX_RUNTIME_DUE_PAGE_ITEMS`,
`AEX_RUNTIME_DUE_PAGE_READS`, `AEX_PRICING_VERSION`.

Two variables refuse the start outright rather than being accepted and ignored:

| Variable | Why it is refused |
| --- | --- |
| `AEX_USAGE_TRANSFER_QUEUE_URL` | The worker holds no transfer-authority binding (OD-25/OD-26). Accepting the variable would leave a live queue URL in the environment for a later change to pick up. |
| `AEX_TRUE_IDLE_MILLIS` | The 180000 ms threshold is a pinned constant. Accepting a tuning variable and ignoring it would let an operator believe they had moved a boundary that never moved (HR-21). |

The engine lives in `aex_runtime_control_aws::worker` and the worker is the thin
deployable over it. One handler serves three entry points, told apart by payload
shape so a single deployable cannot be mis-wired into answering the wrong one:

- an **SQS batch**, answered with a partial-batch response naming exactly the
  failed identifiers;
- a **scheduled sweep** (`{"runtimeSweep": n}`) over one shard of the due index;
- the **internal health surface** (`{"internal": "/internal/readyz"}`). A Lambda
  has no listening socket, so the probe is an invocation answered by the same
  `axum` router an ALB would target — the two cannot answer differently.

The exact-generation fence is `bind_command`, a pure function of the session
pointer: a command naming the current generation proceeds under the pointer's
fence, one naming a superseded generation is settled, and one naming a
generation the authority never allocated is poison. The suspend transition runs
in the required order and each step has its own falsifying test: take the fence,
recount against the session authority, record the intent, dispatch, await,
settle, then emit facts. A recount that disagrees repairs the counter, restores
`running` and suspends nothing on that pass. The eight-hour lifetime is
**active**: `Running -> LifetimeDraining` is a real fenced transition at
`-300 s`, and `-60 s` terminates, closes the receipt and reports
`continuity_lost` with the exact remaining number.

All five production adapters are resolved and probed before polling. The fields
remain optional inside the composition type only so `readyz` and the start
refusal can prove every missing binding is named; production supplies no stub.

### 11.2 `runtimes/hands-agent`

The guest binary serves the five verbs, the attach path, the four provider
lifecycle hooks and the two build hooks on one port — the same port
`CreateMicrovmAuthToken` scopes the endpoint to. There is no shell port and no
second listener. Three variables, none defaulted: `AEX_HANDS_LISTEN_ADDR`,
`AEX_HANDS_JOURNAL_ROOT`, `AEX_HANDS_GUEST_ROOT`, all written by the image from
the same `aex_hands_agent::boot` constants the binary reads.

`/run` and `/resume` bump the incarnation, replay the journal, terminalize every
operation whose process group is gone as `Interrupted`, write the binding and
only then start accepting. A guest with no binding accepts nothing and answers a
plain 503, because before `/run` it cannot encode a response preamble at all. A
malformed run payload fails the launch closed.

Process-group supervision is a `Runner` port, so the whole dispatcher is
exercised off-VM. The POSIX halves refuse rather than guess: a host with no
`/proc` reports that it cannot observe a group instead of reporting extinction,
which would terminalize a live job.

### 11.3 `runtimes/hands-image`

A build tool with four commands: `context`, `build`, `publish` and `validate`.
It writes a `Containerfile` generated from the rootfs contract itself — the
`mkdir` lines come from `ROOTFS_CONTRACT` and a `RUN test ! -e` line from every
entry of `FORBIDDEN_ROOTFS_PATHS` — so the built image and the checked contract
cannot describe different trees.

**The reproducibility pin, and why this one.** The recorded open item is closed
with two pins that are *read*, not guessed:

| Pin | Value | How it was obtained |
| --- | --- | --- |
| Container base, by digest | `public.ecr.aws/lambda/microvms:al2023-minimal@sha256:05cb9b38d841e7ff1b693dc9e894909612f340bf99ec97d426e8000a5bbe96c3` | `docker buildx imagetools inspect`, 2026-08-01 |
| Package source, by date | `--releasever=2023.12.20260629` | the pinned base image's own `/etc/os-release` |

A digest pin alone would not fix the package set, because `dnf` resolves against
a live mirror; a date pin alone would not fix the base, because a tag moves.
AL2023 serves a frozen repository snapshot per `releasever`, so pinning it makes
two builds resolve the same NEVRAs — and taking the value from the base image
rather than from a changelog means the two halves of the build are the same
release rather than two that happen to work together today. The resolved NEVRAs
are then locked in `image.lock.json` and re-checked by `/validate`, so a mirror
that moves anyway fails the build instead of silently changing the image.

`SOURCE_DATE_EPOCH` is fixed, so the double-build check cannot pass or fail on
the clock.

### 11.4 What was earned, and what was not

| Claim | State |
| --- | --- |
| Guest cross-build to `aarch64-unknown-linux-musl` | **Earned.** `file` reports `ELF 64-bit LSB executable, ARM aarch64, statically linked, stripped`, 2 059 448 bytes. The target was installed and linked with `rust-lld`; no C toolchain is in the inputs, because the guest takes the pure-Rust `blake3` on that target. |
| Local `arm64` image build | **Not earned in this run.** The generated `Containerfile` is checked against the rootfs contract by test, the base digest and the `releasever` were both read from the real registry and the real base image, and `docker buildx --platform linux/arm64` was launched and was still resolving the package transaction under QEMU emulation when the wave closed. Nothing was pushed, no `CreateMicrovmImage` was called and no credential was read. The completed build belongs to `aex-live-hands-image` alongside the boot assertions. |
| Everything needing a real `MicroVM` | Unearned and declared, unchanged from §5 and §9. |

### 11.5 Decisions taken in this wave

| ID | Decision | Rationale |
| --- | --- | --- |
| HS-11 | `GenerationState::Resuming` gains `Suspended` as a successor | A `ResumeMicrovm` the provider refuses outright had no effect, so the generation is still suspended. Without the arm one throttle strands the head in `resuming`, which admits nothing and resumes nothing. It is the mirror of the `suspending -> running` restore the suspend transition already relies on. |
| HS-12 | The authoritative open-effect count is its own `OpenEffectCounter` port, not a method on `RuntimeActivityStore` | The two read different tables. Folding them into one trait would let a control-plane adapter silently acquire a session-authority read. |
| HS-13 | `GenerationView` is one bounded read carrying the head, the `MicroVM`, the launch instant, the open accounting interval and the open intent | Reading them as four calls would let the four disagree, and the receipt's `from` cannot be guessed: `RuntimeReceipt::validate` rejects both a gap and an overlap. |
| HS-14 | The worker's readiness and its start refusal are derived from the same five `Option`s | A probe that said "ready" while a port was unbound would be worse than no probe. |
| HS-15 | The internal health surface is reachable by invocation on a Lambda deployable | A Lambda has no socket. Serving the probe from the same router an ALB would target is what keeps one naming rather than two answers. |
| HS-16 | The guest takes pure-Rust `blake3` on `aarch64-unknown-linux-musl` only | A host C compiler in the build inputs is exactly what makes a byte-reproducibility claim untrue. Feature unification is per target, so no other member's build changes. |
| HS-17 | `HostFs` maps the guest root onto a mount point rather than using the path verbatim | On the target the mapping is the identity; off-VM it points at a temporary directory, which is what makes the filesystem matrix evidence for the on-VM behaviour rather than a parallel implementation. |
| HS-18 | The run-hook payload is declared twice — once trusted, once in the guest — with a test comparing the key sets | The guest links no trusted crate (B6), so a shared type is impossible. A drift would fail every launch, and the first place anyone would look is the provider. |

### 11.6 Gaps this wave added or sharpened

| Gap | Handling |
| --- | --- |
| Active lifecycle intent reconciliation | The pure `ReconcileStep` model exists, but the worker does not yet execute it. Existing `dispatched` or `unknown` intents fail closed as `Reconciling`; no second effect is dispatched. This must be closed with the transitional-fence/intent crash window before production readiness is claimed. |
| `Materialize`, `Persist`, `WriteFile` and `Exec` with stdin are refused by the guest | `ContentRef` is a digest and a length; the guest holds no credential and cannot resolve one. Presigned HTTPS would need a TLS client, and the workspace's pinned backend is `aws-lc-rs`, whose crate name the B6 closure scan rejects by prefix. the guest agent's own boundary test asserts no TLS stack is linked, so the choice is checked rather than remembered. |
| `ReadFile` with a byte range is refused | The wire asks for a byte range; `aex-hands-tools` windows by line. Serving the whole file would answer a different question than the caller asked. |
| Attached delivery is not served | `start`'s `Attached` mode is refused explicitly rather than left to hang, so a caller never waits on a body that will not arrive. |
| Pressure release and the orphan sweep are not wired | `plan_release` and `ORPHAN_GRACE_MS` are implemented and tested as pure models. Pressure needs a regional memory-utilization source the provider does not expose, and the orphan sweep needs an "is this `MicroVM` known" lookup the activity port does not offer. Neither is in this wave's scope and both are named here rather than half-built. |
| `cargo fmt --all` cannot run on this host | The invocation exceeds the Windows command-line length limit for a 133-member workspace (`os error 206`). `cargo fmt -p <package>` was run for every owned package. This is a host condition, not a workspace defect. |
