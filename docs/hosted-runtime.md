# Hosted runtime

Aex production is a composition, not a second session engine. Aex Control uses only Brain Server's
versioned network API. Platform pins the Brain image, supplies AWS durability and the AWS
Environment driver, and configures only these trusted capabilities: `aex.output`,
`aex.web.search`, `aex.web.fetch`, and `aex.subagents`. Their scope, retry policy, input ceiling,
and terminal behavior are host configuration; session JSON can select a capability but cannot
widen it.

Production Brain requires DynamoDB, KMS, S3 session storage, and a configured Environment driver,
and fails startup when a required dependency is absent. All initial development and production
resources live in isolated planes in `us-east-1`. The public Brain image also provides explicit
`local` mode using durable SQLite/custody/storage adapters and unsafe host execution. There is no
volatile mode and no production-to-local fallback. Local session storage is restart-safe for
inline objects up to 1 MiB; hosted large-object transfer tickets remain S3-only in the MVP.

## Durability and deletion

Brain commits journal intent before provider or Tool dispatch. DynamoDB stores append-only events
and bounded projections; S3 stores durable session objects and large transfer payloads. Context
compaction is journaled in DynamoDB and reconstructs solely from that append-only journal. A warm
Brain process is only a cache and scheduler. Session storage survives sandbox loss and logical
session end.

`DELETE /v1/sessions/{id}` only crosses a short, durable Aex acceptance boundary and returns `202`
with a `/deletion` status resource. Before Brain can purge anything, the retry worker recursively
ends and fences the session tree, walks Brain's strongly consistent direct-child adjacency, and
settles every journal through its authoritative high-water mark. The SDK's default
`await session.delete()` polls that status with `Retry-After`; `{ queue: true }` returns immediately
after the same acceptance. It never holds one request open across Environment and S3 cleanup. The alpha has
no post-delete content retention: Brain session data, storage objects, and Aex's cached structured
and managed-Tool payloads are purged. Small billing and deletion-status tombstones remain. Bucket
versioning is suspended for new production objects, but deletion remains exhaustive so historical
versions cannot strand data.

## Customer-app Environments

One application connection multiplexes sessions. The SDK obtains a short-lived grant from
`POST /v1/customer-environment/grants` and sends it as the sole `Sec-WebSocket-Protocol` value. API Gateway
forwards callbacks to `POST /v1/customer-environment/gateway`; Aex verifies the private integration on
`$connect`, the trusted proxy path, and the source-IP denylist, then Brain consumes the grant and
owns connection epochs. API Gateway authorizer context is not assumed on later routes. Each runner
registration and heartbeat carries a proof derived from the connect grant, which Brain binds to the
connection and verifies before mutation. An unproved `$disconnect` is advisory and dropped; expiry
comes from the heartbeat lease or a confirmed outbound `410`. There is no credential in the URL and
no tenant header supplied by the caller.

Commands stay below one WebSocket frame. Terminal receipts and results use the scoped HTTPS endpoint
`POST /v1/customer-environment/observations/{grant_id}`. The non-secret grant ID stays in the path and its
paired short-lived token stays only in the public `Authorization` header. On the private Brain hop,
Aex authenticates with its operator bearer and forwards the scoped token separately as
`x-brain-observation-grant`; Brain validates the ID/token pair. Brain reports a missing or ambiguous
delivery honestly to the model; effectful application code should deduplicate `operationId`.

The hosted Brain sends commands with the API Gateway Management API at the HTTPS endpoint in
`BRAIN_CUSTOMER_ENVIRONMENT_CALLBACK_URL`. The AWS SDK's standard retry policy handles retryable gateway
responses. A confirmed `410` fences the recorded connection as gone; a gateway rejection is
unavailable, while a timeout or transport failure is reported as an unknown outcome because the
frame may have arrived.

## Fast prepaid admission

The hot work path reads an in-memory settled-balance cache and per-account reservation. Positive-
balance checks for storage writes use the same cache without adding a compute-sized reservation. A
normal request does not read the SQLite/EFS ledger, replay a session journal, or fetch every session.
A synchronous account fold and durable balance read happen only on a cache miss, stale balance, a
concurrent-ledger fence, or near the configured low-balance threshold; background folds write
absolute idempotent usage.
Concurrent-root-session admission takes the maximum of Brain's eventually consistent tenant/state
index and Aex's durable locally known roots, then briefly caches that conservative count; it never
does one GET per session on the normal create path. Roots in `open`, `ending`, `failed`, or
`deleting` consume a slot because none proves that every sandbox has been released. A strong fold
must observe `ended`, or the deletion worker must observe physical deletion, before the local floor
releases it, so a state-partition gap or failed turn cannot open capacity.
Cross-state index overlap is deduplicated by root ID. Children remain ordinary owned and billable sessions, but only roots
consume this account limit and its hourly-create allowance; Brain's sealed depth, direct-child, and
descendant ceilings govern each tree. A per-account atomic reservation counts `live roots + unique
pending root creates` before any Brain create; concurrent retries sharing one idempotency identity
consume one slot and one hourly-create reservation. Before dispatch, Aex persists the hashed
identity and request digest in SQLite. A definitive pre-commit 4xx rejection releases the claim. A
timeout, transport loss, redirect, server failure, or process crash is ambiguous, so the durable
slot survives until the same idempotency identity proves success or strong discovery observes a new
root. Discovery pairs one such root with at most one unresolved intent to avoid double-counting,
but only an exact same-key response establishes the idempotency mapping. There is no time-only
expiry that could free a committed-but-not-yet-indexed root. The first unambiguous durable response
converts the slot exactly once and invalidates the count for an authoritative refresh. A
per-account single-flight lock prevents a cold burst from duplicating GSI refreshes. Ordinary work
admission state is a bounded second-chance cache, not an account registry. It never evicts live
exposure, process-local create holders, or active refreshes; inactive entries are replaced under a
constant probe budget, and the last refresh caller removes its per-account mutex. Capacity
exhaustion fails closed without a SQLite or Brain call on the ordinary work path; root creation
intentionally crosses the small durable-intent boundary.
The separate retained-root ceiling counts every root whose Brain data has not been physically
deleted, including `ended` and `failed` roots plus uncovered ambiguous create identities. Its SQLite
count and new create intent are one transaction, so racing creates cannot cross the default 100-root
account cap. Ending does not release this capacity; only successful physical deletion does. This
preserves the useful meaning of the smaller concurrent resource-bearing-root limit while giving users an
explicit recovery path: `await session.delete()`. It is defense in depth, not a Dynamo byte bound;
Brain independently enforces 128 MiB per session, 512 MiB cumulative journal data per tenant, and
4,096 retained session identities per tenant at the journal's atomic write boundary.
Separately, `AEX_MAX_CONCURRENT_CREATE_BODIES` defaults to four, bounding simultaneous authenticated
24 MiB create buffers to 96 MiB. `AEX_MAX_CONCURRENT_MESSAGE_BODIES=256` bounds root messages,
child creation prompts, child messages, follow-ups, and the smaller customer-Environment grant/frame/
observation bodies to about 48 MiB of raw buffers at the 192 KiB maximum;
`AEX_MAX_CONCURRENT_INLINE_SESSION_BODIES=64` bounds every other buffered session mutation to
128 MiB of raw 2 MiB buffers. These are independent fail-fast gates so small-message throughput
does not consume large-inline capacity. A saturated route returns `429` before polling the body;
waiting and unauthenticated bodies are not polled. One request holds one slot through its entire
forwarding lifecycle, including any internal recovery work.

Hosted Aex requires a 1-to-128-byte `Idempotency-Key` on operations that can admit new durable work
without an intrinsic resource identity. The SDK generates one and reuses it for its bounded retry:

| Operation | Retry identity |
| --- | --- |
| root session create, root message | required `Idempotency-Key` |
| child create, child message, child follow-up | required `Idempotency-Key` |
| session/child end and session delete | the target session and monotonic lifecycle transition |
| sandbox materialization | the session's single default-sandbox resource |
| durable-storage transfer completion | the minted transfer ID; completion is retry-safe |
| direct sandbox transfer completion | a process-local minted transfer ID; the SDK does not auto-retry ambiguity and callers inspect then prepare fresh |
| storage delete and explicit copies/writes | object/path identity; the SDK does not automatically retry an ambiguous mutation |
| turn cancel/child interrupt | the target session/current turn and monotonic interruption transition |

Brain's neutral API may accept a missing key for local embedding, but the hosted boundary rejects it
with `400 invalid_request`; otherwise a lost response could admit work that a raw caller cannot name
or recover.

Each admitted action reserves a configured maximum-action exposure and the process fails closed when
the per-account reservation ceiling is exhausted. This bounds newly admitted estimated exposure,
not the final charge: an unusually long action can exceed its estimate, so the value must stay above
the operational maximum normally allowed by runtime deadlines. These controls are intentionally in
memory for the alpha. Therefore `aex-control` is an enforced singleton on its SQLite-on-EFS ledger:
desired count is zero or one and releases stop the old task before starting another. Horizontal
control-plane scale is forbidden until the ledger and reservations move to a shared atomic store.
With multiple writers, the exposure ceiling would multiply by replica count.

Background rating discovers Brain-native children through the authoritative reverse-updated tenant
and state indexes. It persists one successful cutoff per account, rereads an overlap across every
non-deleted state, and requests journal deltas only when Brain's `last_seq` advances. The cutoff
moves only after every page and fold succeeds. A process restart therefore retries the same window;
the one-time bootstrap alone may scan old history. The ordinary tick does not load every known
local row: it also settles the bounded set of open turns and the oldest due non-zero session-storage
gauges (five-minute settlement target, 1,000 rows per account per tick). Durable `storage.usage`
transitions reconstruct published-plus-reserved history, including objects created and deleted
entirely between sweeps; HEAD gauges are reconciliation evidence rather than historical samples.
Only transition timestamps advance the durable byte-time integral. The current open interval is
priced ephemerally, so a delayed transition can replace an earlier absolute estimate upward or
downward without double charging. The fold persists a byte-millisecond remainder, so short
intervals are not rounded away on each retry.

The server keeps folds and pricing in integer `i64`/`i128` arithmetic. Public micro-USD values,
cumulative running milliseconds, and storage byte-milliseconds are canonical decimal strings so the
wire remains exact beyond JavaScript's safe-integer range; TypeScript consumers use `BigInt(value)`
for arithmetic. The public storage quantity is the durable closed byte-millisecond integral plus
the derived open interval through that response's `metered_to`, so it reproduces storage micro-USD
exactly as `floor(byte_ms * rate / (1e9 * month_hours * 3_600_000))`. A delayed durable transition
may replace that open estimate upward or downward; the absolute ledger row is overwritten rather
than incremented. Current byte gauges and bounded counters remain JSON numbers only where the
public schema carries an explicit safe maximum.

Explicit balance, usage, refund, and low-balance admission requests perform the rare full local
fold. Up to 16 independent tenant index windows overlap so one remote query does not stall every
account; the singleton SQLite ledger still serializes its short transactions. Deletion runs its
strong subtree settlement before Brain removes a cascading subtree.

Alpha does not charge suspended-sandbox storage. The provider exposes instantaneous lifecycle state
but no authoritative suspension timestamp and retained-byte size, so sampling or predicting the
idle timeout would create a false meter. Aex absorbs snapshot storage and I/O cost for now. The
backlog may reintroduce the line only after the Environment supplies authoritative timestamped size transitions,
or the product explicitly defines a deterministic reserved-storage unit.

Relevant tuning is explicit:

- `AEX_SWEEP_SECONDS`
- `AEX_ADMISSION_ACTION_EXPOSURE_MICROUSD`
- `AEX_ADMISSION_ACCOUNT_EXPOSURE_MICROUSD`
- `AEX_ADMISSION_LOW_BALANCE_MICROUSD`
- `AEX_ADMISSION_STALE_SECONDS`
- `AEX_ADMISSION_RESERVATION_SECONDS`
- `AEX_ADMISSION_MAX_CACHED_ACCOUNTS` (default: 10,000; hard singleton-process tenant-cache bound)
- `AEX_DISCOVERY_OVERLAP_SECONDS` (default: the greater of two sweep intervals or 120 seconds)
- `AEX_DISCOVERY_SESSION_LIMIT` (default: 100,000; bootstrap/runaway safety bound)
- `AEX_DISCOVERY_PARTITIONS` (default: 1; stable Brain changefeed partitions, 1 through 256)
- `AEX_STORAGE_MAX_OBJECT_BYTES` (default: 512 MiB)
- `AEX_STORAGE_MAX_SESSION_BYTES` (default: 10 GiB, including outstanding reservations)
- `AEX_LIMIT_RETAINED_ROOT_SESSIONS` (default: 100; released only by physical deletion)
- `AEX_MAX_CONCURRENT_CREATE_BODIES` (default: 4; 96 MiB aggregate request-buffer ceiling)
- `AEX_MAX_CONCURRENT_MESSAGE_BODIES` (default: 256; about 48 MiB aggregate raw ceiling)
- `AEX_MAX_CONCURRENT_INLINE_SESSION_BODIES` (default: 64; 128 MiB aggregate raw ceiling)

The reservation covers Aex-funded compute and managed services; customers supply provider
credentials, so provider-token charges do not become Aex credit exposure. A process loss discards
the cache, and the first subsequent action reconciles from the durable journal and ledger before it
is admitted.

Storage is not forcibly expired or deleted when an account reaches zero balance. Instead, hosted
Brain atomically enforces `BRAIN_STORAGE_MAX_TENANT_BYTES=10737418240` across published and
upload-reserved bytes in every root and descendant owned by the tenant. This fixed 10 GiB account
ceiling bounds retained unpaid exposure without adding a balance callback to each storage write;
the ordinary balance gate still blocks new writes and work after the settled balance is no longer
positive. Per-object and per-session checks remain defense in depth, while Brain's tenant meter is
the concurrent/crash-safe quota authority.

## Managed-compute capacity

Managed compute is charged against its sealed baseline while a turn is running. Hosted alpha offers
only `1gb`: exactly 0.5 vCPU plus 1 GiB, so the public component rates of $0.19/vCPU-hour and
$0.025/GB-hour produce $0.12/hour. The SDK intentionally exposes no shape selector. A raw root
create asking for another neutral Brain shape returns 422 before Brain dispatch; children and both
default and additional managed sandboxes inherit the same physical seal. Any transient provider
burst or peak capacity is not separately metered and is not a promised autoscaling entitlement.
Aex exposes no peak-capacity meter in the MVP.

The per-account concurrent-root-session limit does not count child sessions and is not a
managed-compute quota: sandboxes materialize lazily, root and child sessions share one default
sandbox, and running plus suspended MicroVM memory counts against the AWS regional quota. The Environment
therefore enforces a separate shared transactional ceiling with `ENVIRONMENT_MAX_MATERIALIZED_MIB`. For
the initial 8-GiB `us-east-1` quota, development is capped at 1 GiB and production at 5 GiB, leaving
2 GiB of regional headroom. A provider capacity rejection is still returned as unavailable; Aex
never falls back to local or another executor.

Within a hosted guest, each custom `.server()` binding gets a bounded generation-lifetime
unprivileged UID. The ordinary shell remains UID 1000 and bindings share workspace access through a
group. The Environment injects declared secrets without writing them to workspace files, argv,
results, or logs, and `/proc` prevents ordinary sibling users from reading them. This is an
unprivileged in-guest boundary, not a defense against guest root or intentional disclosure through
the shared workspace/result. Applications needing that stronger boundary keep the operation in a
`.client()` process or behind an external service. Local mode runs unsandboxed and makes no such
claim.

The official `sandbox` Tool can hold at most two live additional targets per root tree under
`BRAIN_MAX_ADDITIONAL_SANDBOXES_PER_ROOT=2`. The root/child-shared default sandbox is excluded;
terminating an additional target releases its slot while its ID remains a tombstone until root
deletion. This is a fairness guard, not a substitute for the Environment's global memory ceiling, and callers
cannot widen it per session.

Production promotion requires the applied quota and configured image shape to be read from AWS,
the two plane ceilings to fit below it with headroom, and a serialized development launch/resume/
suspend/terminate canary to pass. `AEX_LIMIT_CONCURRENT_SESSIONS` limits roots only and is not
evidence for this gate.

## Required hosted boundaries

- Aex public traffic reaches Brain only with the authenticated tenant sealed in
  `x-brain-tenant-id`; Brain remains authoritative for ownership on the actual session operation.
- The customer-Environment gateway is the only exception: its scoped grant derives tenant and client inside
  Brain, while later frames resolve the stored connection epoch.
- Managed sandbox egress blocks metadata, private/platform ranges, and Aex infrastructure even when
  a session selects public outbound access.
- The Aex-to-control Tool executor listener is private and accepts only the separately configured
  service credential.

## Hosted configuration inventory

The production task role supplies AWS credentials; static AWS access keys are not application
configuration. The initial region is always `AWS_REGION=us-east-1`. These are the required product
values, grouped by the process that reads them:

| Process | Required hosted values |
| --- | --- |
| `aex-control` | `AEX_BRAIN_URL`, `AEX_BRAIN_TOKEN`, `AEX_CONTROL_LISTEN`, `AEX_CONTROL_INTERNAL_LISTEN` (loopback), `AEX_CONTROL_DB` (the singleton EFS path), `AEX_PAYMENTS=stripe`, `STRIPE_SECRET_KEY`, `STRIPE_WEBHOOK_SECRET`, `AEX_TOPUP_SUCCESS_URL`, `AEX_TOPUP_CANCEL_URL`, `AEX_OPERATOR_TOKEN`, `AEX_EXTERNAL_TOOL_EXECUTOR_TOKEN`, `AEX_CUSTOMER_ENVIRONMENT_GATEWAY_TOKEN`, `AEX_CUSTOMER_ENVIRONMENT_TRUSTED_PROXY_CIDRS`, `AEX_MANAGED_ENVIRONMENT_NAT_CIDRS`, and `SERPER_API_KEY` while managed web search is advertised |
| `brain` | `BRAIN_MODE=production`, `BRAIN_LISTEN`, `BRAIN_API_TOKEN`, `BRAIN_EXTERNAL_TOOL_EXECUTOR_URL`, `BRAIN_EXTERNAL_TOOL_EXECUTOR_TOKEN`, `BRAIN_EXTERNAL_TOOL_POLICIES_JSON`, `BRAIN_CUSTOMER_ENVIRONMENT_WEBSOCKET_URL`, `BRAIN_CUSTOMER_ENVIRONMENT_OBSERVATION_BASE_URL`, `BRAIN_CUSTOMER_ENVIRONMENT_CALLBACK_URL`, `BRAIN_JOURNAL_TABLE`, `BRAIN_KMS_KEY_ID`, `BRAIN_SESSION_STORAGE_BUCKET`, `BRAIN_SESSION_STORAGE_PREFIX`, `BRAIN_STORAGE_MAX_OBJECT_BYTES`, `BRAIN_STORAGE_MAX_SESSION_BYTES`, `BRAIN_STORAGE_MAX_TENANT_BYTES`, `BRAIN_STORAGE_TRANSFER_TTL_MS`, `BRAIN_JOURNAL_MAX_SESSION_BYTES`, `BRAIN_JOURNAL_MAX_TENANT_BYTES`, `BRAIN_JOURNAL_MAX_TENANT_SESSIONS`, and `BRAIN_MAX_ADDITIONAL_SANDBOXES_PER_ROOT=2` |
| AWS Environment driver | `ENVIRONMENT_IMAGE`, `ENVIRONMENT_IMAGE_VERSION`, `ENVIRONMENT_REGISTRY_TABLE`, `ENVIRONMENT_MAX_MATERIALIZED_MIB`, `ENVIRONMENT_NETWORK_CONNECTOR_NONE`, `ENVIRONMENT_NETWORK_CONNECTOR_ALLOWLIST`, `ENVIRONMENT_NETWORK_CONNECTOR_PUBLIC`, `ENVIRONMENT_CAPABILITY_SIGNING_KEY_ID`, and `ENVIRONMENT_EGRESS_GATEWAY_AUTHORITY` |

The customer URLs belong only to Brain; control does not read them. The WebSocket URL is the
public API Gateway URL, the observation base is the public Aex HTTPS origin (Brain appends the
non-secret grant path), and the callback URL is the API Gateway Management API HTTPS base.
`BRAIN_EXTERNAL_TOOL_EXECUTOR_URL` and `BRAIN_EXTERNAL_TOOL_EXECUTOR_TOKEN` connect
Brain's fixed `aex.output`, `aex.web.search`, and `aex.web.fetch` capabilities to the private Aex
executor without placing product handlers inside the neutral engine.

Rate-card, admission, discovery, product-limit, Brain scheduling/context, and Environment provider-pacing
variables are optional bounded tuning. Production pins them in infrastructure even when their code
defaults match; changing them is an operational policy change, not a session API feature. The
three `BRAIN_JOURNAL_MAX_*` ceilings are process policy rather than immutable session seals, so
every Brain replica must receive identical values. The
customer-Environment limits bound grant and connection registries, in-flight operations, retained
terminal payloads, and compiled registrations independently so one limit cannot silently consume
the whole Brain heap. The Environment pacing set is
`ENVIRONMENT_PROVIDER_{RUN,RESUME,SUSPEND,TERMINATE}_{RATE_PER_SECOND,BURST}`; there is no Environment storage
bucket or executor-token setting.

The alpha release pins these bounded values explicitly; they are host policy, never public SDK
knobs:

| Process | Pinned bounded values |
| --- | --- |
| `aex-control` | `AEX_SWEEP_SECONDS=30`, `AEX_ADMISSION_ACTION_EXPOSURE_MICROUSD=100000`, `AEX_ADMISSION_ACCOUNT_EXPOSURE_MICROUSD=1000000`, `AEX_ADMISSION_LOW_BALANCE_MICROUSD=1000000`, `AEX_ADMISSION_STALE_SECONDS=60`, `AEX_ADMISSION_RESERVATION_SECONDS=60`, `AEX_ADMISSION_MAX_CACHED_ACCOUNTS=10000`, `AEX_DISCOVERY_OVERLAP_SECONDS=120`, `AEX_DISCOVERY_SESSION_LIMIT=100000`, `AEX_DISCOVERY_PARTITIONS=1`, `AEX_STORAGE_MAX_OBJECT_BYTES=536870912`, `AEX_STORAGE_MAX_SESSION_BYTES=10737418240`, `AEX_MAX_CONCURRENT_CREATE_BODIES=4`, `AEX_MAX_CONCURRENT_MESSAGE_BODIES=256`, `AEX_MAX_CONCURRENT_INLINE_SESSION_BODIES=64`, `AEX_LIMIT_CONCURRENT_SESSIONS=10` (open, ending, failed, or deleting roots), `AEX_LIMIT_RETAINED_ROOT_SESSIONS=100` (all roots until physical deletion), and `AEX_LIMIT_SESSION_CREATES_PER_HOUR=30` (roots) |
| `brain` | `BRAIN_STORAGE_MAX_OBJECT_BYTES=536870912`, `BRAIN_STORAGE_MAX_SESSION_BYTES=10737418240`, `BRAIN_STORAGE_MAX_TENANT_BYTES=10737418240`, `BRAIN_STORAGE_TRANSFER_TTL_MS=900000`, `BRAIN_JOURNAL_MAX_SESSION_BYTES=134217728`, `BRAIN_JOURNAL_MAX_TENANT_BYTES=536870912`, `BRAIN_JOURNAL_MAX_TENANT_SESSIONS=4096`, `BRAIN_MAX_MODEL_ROUNDS=64`, `BRAIN_MAX_TURNS=64`, `BRAIN_PROVIDER_HEADER_TIMEOUT_MS=30000`, `BRAIN_PROVIDER_IDLE_TIMEOUT_MS=60000`, `BRAIN_PROVIDER_TOTAL_TIMEOUT_MS=900000`, `BRAIN_EXTERNAL_TOOL_TIMEOUT_MS=30000`, `BRAIN_MAX_CONCURRENT_CREATES=4`, `BRAIN_MAX_RESIDENT_SESSIONS=128`, `BRAIN_MAX_EVENT_FOLLOWERS=64`, `BRAIN_MAX_ADDITIONAL_SANDBOXES_PER_ROOT=2`, `BRAIN_IDLE_DISCARD_SECONDS=900`, `BRAIN_RECOVERY_POLL_MS=1000`, `BRAIN_RECOVERY_SHARDS_PER_POLL=4`, `BRAIN_RECOVERY_PAGE_SIZE=32`, `BRAIN_MAX_CONCURRENT_RECOVERIES=16`, `BRAIN_CUSTOMER_ENVIRONMENT_MAX_GRANTS=4096`, `BRAIN_CUSTOMER_ENVIRONMENT_MAX_CONNECTIONS=1024`, `BRAIN_CUSTOMER_ENVIRONMENT_MAX_PENDING_OPERATIONS=256`, `BRAIN_CUSTOMER_ENVIRONMENT_MAX_PENDING_TERMINAL_BYTES=33554432`, and `BRAIN_CUSTOMER_ENVIRONMENT_MAX_REGISTRATION_BYTES=67108864` |
| AWS Environment driver | `ENVIRONMENT_BUNDLE_CACHE_MAX_MIB=128`, `ENVIRONMENT_BUNDLE_FETCH_MAX_MIB=32`, plus `RUN=1/1`, `RESUME=5/5`, `SUSPEND=2/2`, and `TERMINATE=10/10` for each `ENVIRONMENT_PROVIDER_*_{RATE_PER_SECOND,BURST}` pair |

The alpha rate card is also sealed in infrastructure:
`AEX_RATE_VCPU_HOUR_MICROUSD=190000`, `AEX_RATE_GB_HOUR_MICROUSD=25000`,
`AEX_RATE_SESSION_STORAGE_GB_MONTH_MICROUSD=30000`,
`AEX_RATE_WEB_SEARCH_QUERY_MICROUSD=3000`, and `AEX_RATE_MONTH_HOURS=730`.

The same provider and external-Tool timeout defaults apply in explicit local mode. They are
process safety bounds, not SDK request deadlines: header and idle timeouts stop stalled streams,
the 15-minute provider total covers the full model attempt, and the external-Tool timeout bounds
one fixed engine-capability call.

The private `platform` repository owns infrastructure, deployment gates, and whole-product release
promotion. A package, backend, or website release alone is not a complete Aex release.
