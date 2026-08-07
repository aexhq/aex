---
title: Tools, managed web, and MCP rewrite handoff
description: Implemented surface, peer contracts, decisions, and remaining work for the tools-mcp stream.
status: accepted
keywords:
  - tool catalog
  - mcp
  - managed web
  - brain
  - descriptors
audience: implementation agents and maintainers
last_verified: 2026-08-01
related:
  - references/rewrite/brain.md
  - references/rewrite/hands.md
---

# Tools, managed web, and MCP handoff

Plan of record: `references/rust-native-rewrite-2026-07-31/plans/09-tools-mcp-web.md` in the parent workspace.

## Implemented

- `aex-brain-tool-catalog` owns a strict manifest vocabulary, the sole-JCS
  canonical catalog encoding, fixed-width low-S P-256 ECDSA verification, and
  the snapshot-asserted 33-entry launch catalog. The catalog includes all five
  Brain-control, eight subagent, two managed-web, seven Hands-filesystem, seven
  Hands-development, and four Hands-browser descriptors. Removed names have no
  alias or route. Hands descriptors have an empty meter set.
- Argument schemas are compiled and validated. Provider/model dependent fields
  and `run_command`'s argument-selected effect variant are tested. The built-in
  digest is
  `sha256:b3cae3e3b5cb64b3ca274f22f67c3ba1e305ac4ca14084967348f06d0ba0fdec`.
- The five control descriptors are present. `todo_read`/`todo_write`, `wait`,
  and `submit_result` have pure bodies. Waits above the remaining budget are
  rejected rather than clamped, and retained results are never truncated.
  Approval creation remains an application concern because it requires the
  peer session store and the full eleven-field binding.
- Readiness is fail-closed: duplicate names, zero/multiple executor claims,
  unknown exact selections, missing exact capabilities/credentials, and
  approval-policy names outside the advertised set are typed failures. Default
  composition honestly omits optional `web_search` and browser authority.
- `CompositeToolRouter` implements the peer `aex_brain_app::ToolPort`.
  Routes are resolved only from an exact immutable `CatalogPin`; duplicate
  executors/catalogs/tools fail at construction. Detached query and cancel take
  the durable `DetachedOperationRef { id, executor }`, address that executor's
  fixed slot directly, and keep no process-local operation registry or lock.
  Equal raw ids from different executors therefore cannot collide or broadcast.
- `aex-brain-managed-web::egress` enforces absolute HTTPS, port 443, no
  userinfo, a URL-size bound, the complete IPv4/IPv6 deny table, embedded IPv4
  unwrapping, full-record-set screening, and exact screened-address pinning.
  Redirects are manual, capped at five, loop checked, and re-run the whole
  parse/resolve/screen/pin path at every hop.
- `web_fetch` has connect, TTFB, progress, and total deadlines; a closed media
  allowlist; explicit gzip/Brotli decompression with byte and 100:1 ratio
  bounds; strict no-truncation behavior; content hashes; and deterministic
  `html5ever` HTML-to-Markdown/text serialization with a 100-run stability
  assertion.
- `web_search` accepts only the `aex_web_search` workspace-secret shape with
  the closed Brave/Serper enum and pinned endpoints. Secret material is
  zeroized/redacted, arbitrary endpoint overrides are unrepresentable, and
  recorded provider fixtures normalize in provider order while accounting for
  invalid and excess rows.
- `aex-brain-mcp` pins exactly `rmcp =3.1.0` with only the client,
  streamable-HTTP-client, and reqwest features. It emits the exact
  `2026-07-28` protocol identity and Base64 sentinel header encoding, derives
  `Mcp-Param-*` from the same canonical arguments as the body, validates
  `x-mcp-header` placement/type/name/uniqueness, bounds discovery and response
  frames, and never represents session, GET-stream, or Last-Event-ID state.
- MCP recovery is Task identity or nothing: only `NotSent` can retry the same
  effect; an observed Task is queried by the same server-scoped id; no Task id
  is `OutcomeUnknown`; working Tasks schedule a 1–30 second durable wake;
  input-required cancels then fails definitively; TTL expiry stays unknown.

## Public peer surface

The principal published paths are:

- `aex_brain_domain::effect::DetachedOperationRef`; this is the durable,
  executor-qualified recovery address shared by effect evidence, journal waits,
  `ToolPort`, and the composite router.
- `aex_brain_tool_catalog::manifest::{ToolName, ToolDescriptor,
  ToolManifestEntry, ToolBoundary, RecoveryClass, ApprovalPolicy, ToolBounds,
  UsageDimensionSet, EgressClass, CredentialClass, Determinism,
  CapabilityRequirement, EntryState, ExclusionReason}`
- `aex_brain_tool_catalog::catalog::{builtin_entries,
  builtin_catalog_bytes, validate_arguments, select_effect,
  BUILTIN_CATALOG_DIGEST}`
- `aex_brain_tool_catalog::signature::{CatalogSignature, SignatureAlg,
  SigningKeyId, VerificationKey, TrustStore, catalog_signing_input,
  verify_catalog_signature}`
- `aex_brain_tool_catalog::readiness::{BuiltinSelection, ExecutorRegistry,
  CapabilitySet, ResolvedSecretNames, ReadinessInput, AdvertisedCatalog,
  ReadinessFailure, advertise}`
- `aex_brain_tool_catalog::router::{CompositeToolRouter, ToolExecutor,
  RouterBuildError}`
- `aex_brain_tool_catalog::control::{TodoItem, TodoStatus, TodoCounts,
  TodoReadResult, TodoWriteResult, WaitPlan, PreparedSubmitResult,
  ControlFailure, apply_todo_write, render_todo_read, prepare_wait,
  prepare_submit_result}`
- `aex_brain_managed_web::egress::{EgressPolicy, ParsedTarget,
  ValidatedTarget, DnsResolver, SystemDnsResolver, EgressRejection, validate,
  resolve_and_screen}`
- `aex_brain_managed_web::fetch::{FetchFormat, FetchRequest, FetchDocument,
  FetchRejection, fetch, format_document, media_type_allowed}`
- `aex_brain_managed_web::serializer::{html_to_markdown, html_to_text}`
- `aex_brain_managed_web::search::{WEB_SEARCH_SECRET_NAME,
  WebSearchProviderId, WebSearchCredential, SearchFreshness,
  WebSearchRequest, SearchResult, WebSearchResult, SearchRejection, search,
  normalize_brave, normalize_serper}`
- `aex_brain_mcp::client::{PROTOCOL_REVISION,
  StreamableHttpClientTransport, McpHeaderAnnotations, McpHeaders,
  DiscoveredTool, QualifiedTool, ExcludedTool, McpServerManifest,
  ToolExclusionReason, QualificationError, ResponseBounds,
  ResponseBoundError, qualify_discovery}`
- `aex_brain_mcp::recovery::{TaskCapability, McpTaskId, TaskState,
  TaskTransition, DropRecovery, after_stream_drop, task_transition}`

## Temporary peer types and required peer changes

Two temporary catalog types remain, both marked at their definitions:

- `aex_brain_tool_catalog::wire_pending::ExecutorRoute` needs a peer route
  vocabulary that distinguishes control, park, subagent scheduler, the three
  Hands families, and registered custom routes. The landed
  `aex_brain_domain::journal::ExecutorRoute` intentionally records only the
  coarse executed-on receipt and cannot replace this manifest route.
- `aex_brain_tool_catalog::wire_pending::DurableOperationSupport` needs a
  catalog-level `None | Query` vocabulary. The landed Brain temporary model
  capability uses `None | Proven` and is not the same decision.

The merge consumed the now-landed peer types
`aex_brain_domain::EffectClass` and `aex_brain_domain::DispatchProof`; the MCP
temporary copy was deleted.

The frozen workspace has no `aex-live-managed-web` or `aex-live-mcp` member
under `tests/live`. This stream did not add/rename members, per
the orchestrator rule. The existing crate metadata points at
`aex-live-brain-mux`; the orchestrator must either allocate the two named
companions in a member-set wave or explicitly confirm the combined companion.

## Deliberately deferred

- A registrar-facing, KMS-signable `ToolCatalogRevision`/dependency-closure
  document is not invented without the still-missing peer encryption-domain,
  archive-validation, compiled-form, and registry-pointer types. The built-in
  revision is immutable and snapshot/digest bound; signature verification for
  registered revisions is implemented.
- The full networked `McpAdapter::{qualify, call, task_get, task_cancel}` and
  tenant/secret-generation connection pools remain for the mux/registrar
  integration wave. This crate supplies the pinned transport type, qualification
  validation, headers, bounds, and Task recovery state machine, but makes no
  claim of a live-server conformance receipt.
- Exact encoded request-byte usage facts and retained memory/compute facts need
  the landed usage sink and effect-attempt identity at the mux composition
  boundary. No approximate fact is emitted here.
- Fetch decoding currently uses the declared HTTP charset and then UTF-8. The
  first-1024-byte HTML meta-charset fallback is not yet added.
- Standalone live managed-web/MCP tests are blocked by the frozen missing
  companions above. No unit test self-skips or substitutes a real provider.

## Decisions

- Built-ins remain unsigned at runtime and are bound by the compiled digest;
  only external registered/MCP revisions use P-256 `ECDSA_SHA_256`.
- The exact plan table makes `create_subagent` a zero-weight scheduler action;
  catalog validation treats the subagent scheduler as a durable-park class for
  the V4 zero-weight rule.
- Optional authority affects advertisement, never call-time fallback. An exact
  selection upgrades the same absence to a readiness error.
- Search provider status errors contain only the numeric HTTP status; provider
  bodies and keys never enter an error or result.
- Fetch disables reqwest auto-decompression so encoded and decoded bounds are
  enforced by AEX-owned code, and sends no ambient proxy, cookie, authorization,
  or proxy-authorization state.
