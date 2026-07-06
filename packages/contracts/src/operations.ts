import { strToU8, zipSync } from "fflate";
import { randomUUID } from "node:crypto";
import type { HttpClient } from "./http.js";
import type { AexEvent } from "./event-envelope.js";
import type { RunUnit } from "./run-unit.js";
import { normalizeRunUnit } from "./run-unit.js";
import { RunConfigValidationError, RunStateError } from "./sdk-errors.js";
import {
  assertRunRecordArchivePublicSafeV1,
  buildRunRecordDownloadManifestV1,
  type RunRecordArchiveEntryForRedactionV1,
  type RunRecordArtifactSummaryV1,
  type RunRecordDownloadErrorV1
} from "./run-record.js";
import type { RunCostTelemetry } from "./run-cost.js";
import type {
  AgentsMdRecord,
  BillingCheckoutRequest,
  BillingHostedSession,
  BillingLedgerPage,
  BillingLedgerQuery,
  BillingPortalRequest,
  BillingSummary,
  ChildRunRef,
  FileRecord,
  Output,
  OutputLink,
  OutputLinkOptions,
  OutputFileDownload,
  OutputFilePathSelector,
  OutputFileSelector,
  OutputFileType,
  OutputQuery,
  OutputText,
  ReadOutputTextOptions,
  Run,
  RunListPage,
  RunListQuery,
  RunSummary,
  Session,
  SessionCreateRequest,
  SessionEvent,
  SessionListPage,
  SessionListQuery,
  SessionMessageAccepted,
  SessionMessageRequest,
  SessionMessagesPage,
  SessionMessagesQuery,
  SessionStateChangeAccepted,
  RunWebhookDelivery,
  SecretRecord,
  SecretReveal,
  SkillRecord,
  WebhookSigningSecret,
  WhoAmI
} from "./runtime-types.js";
import type { PlatformRunSubmissionInput, PlatformSubmission } from "./submission.js";

/**
 * The single source of truth for SDK<->BFF transport. The SDK class
 * AND the CLI subcommands both call these functions; neither
 * surface re-implements HTTP requests against the dashboard.
 *
 * Every function takes an HttpClient (so callers control auth + fetch
 * injection) and returns parsed responses.
 *
 * Workspace identity is derived server-side from the API key on
 * every request; callers do not pass `workspaceId`.
 */

export async function getRun(http: HttpClient, runId: string): Promise<Run> {
  const result = await http.request<Run | { readonly run: Run }>(
    `/api/runs/${encodeURIComponent(runId)}`
  );
  return hasRun(result) ? result.run : result;
}

/**
 * Strongly-typed accessor for the full self-contained run unit:
 * parsed submission inputs, attempts, indexed events (with
 * pagination cursor for large runs), raw-event Storage manifest,
 * outputs, capture failures, and the proxy-call audit.
 *
 * Backed by the same `GET /api/runs/:runId` endpoint that
 * `getRun` calls; this variant just narrows the return type to
 * the documented wire shape. Prefer this for new code; `getRun`
 * stays for callers that only need the loose record.
 */
export async function getRunUnit(http: HttpClient, runId: string): Promise<RunUnit> {
  // Normalize so the RunUnit type contract holds at runtime: the managed plane
  // returns a lean record and omits the aggregate collections (F25). The
  // aggregates default to empty (safe array/page access) — read outputs()/events()
  // for the authoritative per-run data on that plane.
  return normalizeRunUnit(await http.request<unknown>(`/api/runs/${encodeURIComponent(runId)}`));
}

/**
 * List the runs in the token's workspace, most-recent first, one page at a time.
 * Backed by `GET /api/runs` (workspace-token gated; the bare collection path, NOT
 * the run-keyed `GET /api/runs/:runId`). The server clamps `limit` to [1, 100] and
 * returns an opaque `nextCursor` for the next page (absent on the last page).
 *
 * Returns public-safe {@link RunSummary} rows only — never the submission snapshot.
 * For a single page; callers wanting every run loop on `nextCursor` themselves.
 */
export async function listRuns(http: HttpClient, query?: RunListQuery): Promise<RunListPage> {
  const params: Record<string, string> = {};
  if (query?.status !== undefined) params.status = query.status;
  if (query?.since !== undefined) params.since = query.since;
  if (query?.limit !== undefined) params.limit = String(query.limit);
  if (query?.cursor !== undefined) params.cursor = query.cursor;
  const page = await http.request<RunListPage>("/api/runs", {}, params);
  // Defensive contract enforcement: some deployed planes leak non-run marker
  // rows (settle-time ledger/spendmark items) into the run-list index. Those
  // phantoms carry only { id, createdAt } and would surface as duplicate,
  // status-less RunSummary entries. Drop anything missing the fields
  // RunSummary declares required, so callers can trust the published type.
  // The same enforcement covers `costUsd`: deployed planes serve `null` for
  // runs with no settled telemetry, but RunSummary declares `costUsd?: number`
  // — normalize `null` to absent so typed callers never see it.
  let changed = false;
  const runs: RunSummary[] = [];
  for (const run of page.runs) {
    if (
      typeof run.id !== "string" ||
      typeof run.status !== "string" ||
      typeof run.createdAt !== "string" ||
      typeof run.updatedAt !== "string"
    ) {
      changed = true;
      continue;
    }
    if (typeof run.costUsd !== "number" && run.costUsd !== undefined) {
      const { costUsd: _dropped, ...rest } = run;
      runs.push(rest);
      changed = true;
      continue;
    }
    runs.push(run);
  }
  return changed ? { ...page, runs } : page;
}

export interface IdempotencyOptions {
  readonly idempotencyKey?: string;
}

export interface SubmitOptions extends IdempotencyOptions {
  readonly messageIdempotencyKey?: string;
}

/**
 * Resolve a caller-supplied idempotency key to the value that ships on the
 * request. FAIL-FAST: an empty or whitespace-only key THROWS
 * {@link RunConfigValidationError} — a footgun that silently disabled dedup
 * (`?? generate()` kept `''`, then a downstream truthy header-drop shipped no
 * `Idempotency-Key`). An absent key generates a fresh one; a real key is
 * returned verbatim. The single choke point every send/create/run entry uses.
 */
export function resolveIdempotencyKey(key?: string): string {
  if (key === undefined) {
    return `aex-idem-${randomUUID()}`;
  }
  if (typeof key !== "string" || key.trim().length === 0) {
    throw new RunConfigValidationError("idempotencyKey must be a non-empty, non-whitespace string", {
      field: "idempotencyKey",
      value: key
    });
  }
  return key;
}

/**
 * Fail-closed idempotency header builder. An EMPTY string throws (defense in
 * depth alongside {@link resolveIdempotencyKey}) rather than silently dropping
 * the header and proceeding non-idempotent; an absent key yields no header.
 */
function idempotencyHeaders(options?: IdempotencyOptions): HeadersInit | undefined {
  if (options?.idempotencyKey === undefined) return undefined;
  if (typeof options.idempotencyKey !== "string" || options.idempotencyKey.trim().length === 0) {
    throw new RunConfigValidationError("idempotencyKey must be a non-empty, non-whitespace string", {
      field: "idempotencyKey",
      value: options.idempotencyKey
    });
  }
  return { "Idempotency-Key": options.idempotencyKey };
}

export async function createSession(
  http: HttpClient,
  request: SessionCreateRequest,
  options?: IdempotencyOptions
): Promise<Session> {
  const headers = idempotencyHeaders(options);
  const result = await http.request<Session | { readonly session: Session }>("/api/sessions", {
    method: "POST",
    ...(headers ? { headers } : {}),
    body: JSON.stringify(request)
  });
  return unwrapSession(result);
}

/** The result of a non-blocking {@link submit}: the run id + the created session. */
export interface SubmitResult {
  readonly runId: string;
  readonly session: Session;
}

/**
 * Fire-and-forget submit — create the session and post its first turn WITHOUT
 * awaiting the turn to settle (the honest counterpart to await-settle `run()`).
 * Returns the `runId` immediately; observe the run via a `webhook`, the event
 * stream, or by re-opening the session. Mirrors {@link createSession}'s
 * idempotency handling.
 */
export async function submit(
  http: HttpClient,
  request: SessionCreateRequest,
  options?: SubmitOptions
): Promise<SubmitResult> {
  const createKey = resolveIdempotencyKey(options?.idempotencyKey);
  const messageKey =
    options?.messageIdempotencyKey !== undefined
      ? resolveIdempotencyKey(options.messageIdempotencyKey)
      : `${createKey}:message`;
  const { input, ...createRequest } = request;
  assertSubmitInput(input);

  const created = await createSession(http, createRequest, { idempotencyKey: createKey });
  const sessionId = created.sessionId ?? created.id;
  const accepted = await sendSessionMessage(http, sessionId, { input }, { idempotencyKey: messageKey });
  const session = accepted.session;
  return { runId: session.sessionId ?? session.id, session };
}

function assertSubmitInput(input: SessionCreateRequest["input"]): asserts input is string | readonly string[] {
  const ok =
    (typeof input === "string" && input.length > 0) ||
    (Array.isArray(input) && input.length > 0 && input.every((segment) => typeof segment === "string" && segment.length > 0));
  if (!ok) {
    throw new RunConfigValidationError("submit: request.input must be a non-empty string or string array", {
      field: "input",
      value: input
    });
  }
}

export async function getSession(http: HttpClient, sessionId: string): Promise<Session> {
  const result = await http.request<Session | { readonly session: Session }>(
    `/api/sessions/${encodeURIComponent(sessionId)}`
  );
  return unwrapSession(result);
}

export async function listSessions(
  http: HttpClient,
  query?: SessionListQuery
): Promise<SessionListPage> {
  const params: Record<string, string> = {};
  if (query?.status !== undefined) params.status = query.status;
  if (query?.since !== undefined) params.since = query.since;
  if (query?.limit !== undefined) params.limit = String(query.limit);
  if (query?.cursor !== undefined) params.cursor = query.cursor;
  return http.request<SessionListPage>("/api/sessions", {}, params);
}

export async function sendSessionMessage(
  http: HttpClient,
  sessionId: string,
  request: SessionMessageRequest,
  options?: IdempotencyOptions
): Promise<SessionMessageAccepted> {
  const headers = idempotencyHeaders(options);
  return http.request<SessionMessageAccepted>(
    `/api/sessions/${encodeURIComponent(sessionId)}/messages`,
    {
      method: "POST",
      ...(headers ? { headers } : {}),
      body: JSON.stringify(request)
    }
  );
}

export async function listSessionMessages(
  http: HttpClient,
  sessionId: string,
  query?: SessionMessagesQuery
): Promise<SessionMessagesPage> {
  const params: Record<string, string> = {};
  if (query?.limit !== undefined) params.limit = String(query.limit);
  if (query?.cursor !== undefined) params.cursor = query.cursor;
  if (query?.since !== undefined) params.since = query.since;
  return http.request<SessionMessagesPage>(
    `/api/sessions/${encodeURIComponent(sessionId)}/messages`,
    {},
    params
  );
}

export async function suspendSession(
  http: HttpClient,
  sessionId: string,
  options?: IdempotencyOptions
): Promise<SessionStateChangeAccepted> {
  const headers = idempotencyHeaders(options);
  return http.request<SessionStateChangeAccepted>(
    `/api/sessions/${encodeURIComponent(sessionId)}/suspend`,
    { method: "POST", ...(headers ? { headers } : {}) }
  );
}

export async function cancelSession(
  http: HttpClient,
  sessionId: string,
  options?: IdempotencyOptions
): Promise<SessionStateChangeAccepted> {
  const headers = idempotencyHeaders(options);
  return http.request<SessionStateChangeAccepted>(
    `/api/sessions/${encodeURIComponent(sessionId)}/cancel`,
    { method: "POST", ...(headers ? { headers } : {}) }
  );
}

export async function resumeSession(
  http: HttpClient,
  sessionId: string,
  options?: IdempotencyOptions
): Promise<SessionStateChangeAccepted> {
  const headers = idempotencyHeaders(options);
  return http.request<SessionStateChangeAccepted>(
    `/api/sessions/${encodeURIComponent(sessionId)}/resume`,
    { method: "POST", ...(headers ? { headers } : {}) }
  );
}

/**
 * Request the HITL write-gate: park the session `awaiting_approval` before its
 * next gated action (mirrors {@link suspendSession}). Imperative counterpart to
 * the declarative submission-time `approvalGate`.
 */
export async function requestApproval(
  http: HttpClient,
  sessionId: string,
  options?: IdempotencyOptions
): Promise<SessionStateChangeAccepted> {
  const headers = idempotencyHeaders(options);
  return http.request<SessionStateChangeAccepted>(
    `/api/sessions/${encodeURIComponent(sessionId)}/request-approval`,
    { method: "POST", ...(headers ? { headers } : {}) }
  );
}

/** Approve an `awaiting_approval` session so the held turn resumes (→ running). */
export async function approveSession(
  http: HttpClient,
  sessionId: string,
  options?: IdempotencyOptions
): Promise<SessionStateChangeAccepted> {
  const headers = idempotencyHeaders(options);
  return http.request<SessionStateChangeAccepted>(
    `/api/sessions/${encodeURIComponent(sessionId)}/approve`,
    { method: "POST", ...(headers ? { headers } : {}) }
  );
}

/** Deny an `awaiting_approval` session so the held turn is cancelled (→ cancelled). */
export async function denySession(
  http: HttpClient,
  sessionId: string,
  options?: IdempotencyOptions
): Promise<SessionStateChangeAccepted> {
  const headers = idempotencyHeaders(options);
  return http.request<SessionStateChangeAccepted>(
    `/api/sessions/${encodeURIComponent(sessionId)}/deny`,
    { method: "POST", ...(headers ? { headers } : {}) }
  );
}

export async function deleteSession(
  http: HttpClient,
  sessionId: string,
  options?: IdempotencyOptions
): Promise<SessionStateChangeAccepted | void> {
  const headers = idempotencyHeaders(options);
  return http.request<SessionStateChangeAccepted | void>(
    `/api/sessions/${encodeURIComponent(sessionId)}`,
    { method: "DELETE", ...(headers ? { headers } : {}) }
  );
}

export async function listSessionEvents(
  http: HttpClient,
  sessionId: string
): Promise<readonly SessionEvent[]> {
  const path = `/api/sessions/${encodeURIComponent(sessionId)}/events`;
  const all: SessionEvent[] = [];
  let cursor: number | undefined;
  for (let page = 0; page < LIST_EVENTS_PAGE_BUDGET; page++) {
    const query = cursor !== undefined ? { cursor: String(cursor) } : {};
    const result = await http.request<{ readonly events: readonly SessionEvent[]; readonly nextCursor?: number | null }>(
      path,
      {},
      query
    );
    all.push(...result.events);
    if (typeof result.nextCursor !== "number") break;
    cursor = result.nextCursor;
  }
  return all;
}

export async function listSessionOutputs(
  http: HttpClient,
  sessionId: string,
  query?: OutputQuery
): Promise<readonly Output[]> {
  const result = await http.request<{ readonly outputs: readonly Output[] }>(
    `/api/sessions/${encodeURIComponent(sessionId)}/outputs`
  );
  return query === undefined ? result.outputs : filterOutputs(result.outputs, query);
}

export async function getSessionCoordinatorTicket(
  http: HttpClient,
  sessionId: string
): Promise<CoordinatorTicket> {
  return http.request<CoordinatorTicket>(
    `/api/sessions/${encodeURIComponent(sessionId)}/events/ticket`,
    { method: "POST" }
  );
}

// Bound the transparent pager: the read route caps each page at 1000, so this
// admits up to ~1e6 events before bailing — past any real run, but bounded so a
// server that never clears `nextCursor` can't loop forever.
const LIST_EVENTS_PAGE_BUDGET = 1000;

/**
 * List a run's events. The read endpoint is PAGED (bounded per response so a
 * long run can't return an unbounded body); this follows `nextCursor` across
 * pages and returns the FULL accumulated list, preserving the prior single-call
 * contract for callers (download/*, CLI, streamEvents polling).
 */
export async function listRunEvents(
  http: HttpClient,
  runId: string
): Promise<readonly AexEvent[]> {
  const path = `/api/runs/${encodeURIComponent(runId)}/events`;
  const all: AexEvent[] = [];
  let cursor: number | undefined;
  for (let page = 0; page < LIST_EVENTS_PAGE_BUDGET; page++) {
    const query = cursor !== undefined ? { cursor: String(cursor) } : {};
    const result = await http.request<{ readonly events: readonly AexEvent[]; readonly nextCursor?: number | null }>(
      path,
      {},
      query
    );
    all.push(...result.events);
    if (typeof result.nextCursor !== "number") break;
    cursor = result.nextCursor;
  }
  return all;
}

/** A coordinator WS connection grant minted by the hosted API's ticket broker. */
export interface CoordinatorTicket {
  readonly wsUrl: string;
  readonly ticket: string;
  readonly expiresAtMs: number;
}

/**
 * Mint a short-lived coordinator WS ticket via the workspace-token-gated
 * broker (`/api/runs/:id/events/ticket`). The returned `wsUrl` + `ticket`
 * open the live event stream directly against the coordinator. Throws if no
 * coordinator is configured for the deployment (HTTP 503).
 */
export async function getCoordinatorTicket(http: HttpClient, runId: string): Promise<CoordinatorTicket> {
  return http.request<CoordinatorTicket>(
    `/api/runs/${encodeURIComponent(runId)}/events/ticket`,
    { method: "POST" }
  );
}

export async function listOutputs(
  http: HttpClient,
  runId: string,
  query?: OutputQuery
): Promise<readonly Output[]> {
  const result = await http.request<{ readonly outputs: readonly Output[] }>(
    `/api/runs/${encodeURIComponent(runId)}/outputs`
  );
  return query === undefined ? result.outputs : filterOutputs(result.outputs, query);
}

export async function findOutputs(
  http: HttpClient,
  runId: string,
  query: OutputQuery
): Promise<readonly Output[]> {
  return listOutputs(http, runId, query);
}

export async function findOutput(
  http: HttpClient,
  runId: string,
  query: OutputQuery
): Promise<Output | null> {
  const matches = await findOutputs(http, runId, query);
  if (matches.length === 0) return null;
  if (matches.length === 1) return matches[0]!;
  throw new RunStateError("outputs.findOne: output query matched multiple files", {
    runId,
    matches: matches.map((output) => output.filename ?? output.id)
  });
}

export type OutputLinkSelector = string | OutputFileSelector | OutputQuery;

export async function outputLink(
  http: HttpClient,
  runId: string,
  selectorOrQuery: OutputLinkSelector,
  options?: OutputLinkOptions
): Promise<OutputLink> {
  const output = await resolveOutputLinkTarget(http, runId, selectorOrQuery);
  const expiresInSeconds = normalizeOutputLinkExpiresIn(options?.expiresIn);
  const result = await http.request<OutputLink>(
    `/api/runs/${encodeURIComponent(runId)}/outputs/${encodeURIComponent(output.id)}/link`,
    {
      method: "POST",
      body: JSON.stringify({ expiresInSeconds })
    }
  );
  const effectiveExpiresIn = result.expiresInSeconds ?? expiresInSeconds;
  return {
    ...result,
    expiresInSeconds: effectiveExpiresIn,
    expiresAt: result.expiresAt ?? syntheticExpiresAt(effectiveExpiresIn),
    output: result.output ?? output
  };
}

/**
 * The hosted API returns `{ url, expiresInSeconds }` without an absolute
 * timestamp; the documented `link.expiresAt` is synthesized client-side from
 * the mint time so it is always present on a returned link.
 */
function syntheticExpiresAt(expiresInSeconds: number): string {
  return new Date(Date.now() + expiresInSeconds * 1000).toISOString();
}

export async function createOutputLink(
  http: HttpClient,
  runId: string,
  selectorOrQuery: OutputLinkSelector,
  options?: OutputLinkOptions
): Promise<OutputLink> {
  return outputLink(http, runId, selectorOrQuery, options);
}

export async function eventArchiveLink(
  http: HttpClient,
  runId: string,
  options?: OutputLinkOptions
): Promise<OutputLink> {
  const expiresInSeconds = normalizeOutputLinkExpiresIn(options?.expiresIn);
  const result = await http.request<OutputLink>(
    `/api/runs/${encodeURIComponent(runId)}/events/link`,
    {
      method: "POST",
      body: JSON.stringify({ expiresInSeconds })
    }
  );
  const effectiveExpiresIn = result.expiresInSeconds ?? expiresInSeconds;
  return {
    ...result,
    expiresInSeconds: effectiveExpiresIn,
    expiresAt: result.expiresAt ?? syntheticExpiresAt(effectiveExpiresIn)
  };
}

export function resolveOutputFileSelector(
  outputs: readonly Output[],
  selector: OutputFileSelector,
  runId?: string
): Output {
  if (isPathSelector(selector)) {
    const target = normalizeOutputLookupPath(selector.path);
    if (!target) {
      throw new RunStateError("outputs.download: output path must be non-empty", { runId, path: selector.path });
    }
    const matches = outputs.filter((output) => {
      if (typeof output.filename !== "string") return false;
      const filename = normalizeOutputLookupPath(output.filename);
      if (selector.match === "suffix") {
        return filename === target || filename.endsWith(`/${target}`);
      }
      return filename === target;
    });
    if (matches.length === 1) return matches[0]!;
    if (matches.length > 1) {
      throw new RunStateError(
        `outputs.download: output path "${selector.path}" matched multiple files`,
        { runId, path: selector.path, matches: matches.map((output) => output.filename ?? output.id) }
      );
    }
    throw new RunStateError(`outputs.download: output path "${selector.path}" was not found`, {
      runId,
      path: selector.path
    });
  }
  if (typeof selector?.id !== "string" || selector.id.length === 0) {
    throw new RunStateError("outputs.download: selector must include an output id or path", { runId });
  }
  return { ...selector, id: selector.id };
}

export async function downloadOutput(
  http: HttpClient,
  runId: string,
  selector: OutputFileSelector
): Promise<OutputFileDownload> {
  const output = isPathSelector(selector)
    ? resolveOutputFileSelector(await listOutputs(http, runId), selector, runId)
    : resolveOutputFileSelector([], selector, runId);
  const { response } = await http.download(
    `/api/runs/${encodeURIComponent(runId)}/outputs/${encodeURIComponent(output.id)}/download`
  );
  return { output, bytes: new Uint8Array(await response.arrayBuffer()) };
}

/** Byte ceiling for {@link readOutputText} — a hard cap even if a caller asks for more. */
export const READ_OUTPUT_TEXT_MAX_BYTES = 10_000_000;
/** Default `maxBytes` for {@link readOutputText} — a chat-sized preview. */
export const READ_OUTPUT_TEXT_DEFAULT_BYTES = 50_000;

/**
 * Read ONE output file as byte-capped, decoded UTF-8 text. Built for handing a run
 * deliverable to an LLM tool: it streams the file body and STOPS at `maxBytes`, so
 * a 200 MB artifact never fully buffers in memory or context. `truncated` is true
 * when the file is larger than the cap. Optionally `grep` keeps only matching lines.
 *
 * Selector is the same `{ path }` / `{ id }` shape as `downloadOutput`. A path
 * selector lists the run's outputs to resolve the id; an id selector skips that.
 */
export async function readOutputText(
  http: HttpClient,
  runId: string,
  selector: OutputFileSelector,
  options?: ReadOutputTextOptions
): Promise<OutputText> {
  const maxBytes = Math.max(1, Math.min(options?.maxBytes ?? READ_OUTPUT_TEXT_DEFAULT_BYTES, READ_OUTPUT_TEXT_MAX_BYTES));
  const output = isPathSelector(selector)
    ? resolveOutputFileSelector(await listOutputs(http, runId), selector, runId)
    : resolveOutputFileSelector([], selector, runId);
  const { response } = await http.download(
    `/api/runs/${encodeURIComponent(runId)}/outputs/${encodeURIComponent(output.id)}/download`
  );
  const capped = await readCappedText(response, maxBytes);
  const text = options?.grep === undefined ? capped.text : grepLines(capped.text, options.grep);
  return { output, text, truncated: capped.truncated, totalBytes: capped.totalBytes };
}

/**
 * Read a streamed response body up to `maxBytes`, decode as UTF-8, and report
 * whether the file was larger than the cap. Prefers the `content-length` header
 * for `totalBytes`; falls back to the bytes actually read. Cancels the stream
 * once the cap is reached so the remainder is never transferred.
 */
async function readCappedText(
  response: Response,
  maxBytes: number
): Promise<{ readonly text: string; readonly truncated: boolean; readonly totalBytes: number }> {
  const declaredRaw = response.headers.get("content-length");
  const declared = declaredRaw !== null && /^\d+$/.test(declaredRaw) ? Number(declaredRaw) : undefined;
  const decoder = new TextDecoder("utf-8");
  const body = response.body;
  if (!body) {
    // No streaming body (some fetch polyfills) — buffer, then slice to the cap.
    const buf = new Uint8Array(await response.arrayBuffer());
    const total = declared ?? buf.byteLength;
    return {
      text: decoder.decode(buf.subarray(0, maxBytes)),
      truncated: buf.byteLength > maxBytes,
      totalBytes: total
    };
  }
  const reader = body.getReader();
  const chunks: Uint8Array[] = [];
  let read = 0;
  let sawMore = false;
  try {
    while (read < maxBytes) {
      const { done, value } = await reader.read();
      if (done) break;
      if (value && value.byteLength > 0) {
        read += value.byteLength;
        chunks.push(value);
      }
    }
    if (read >= maxBytes) {
      // We hit the cap; peek once more to learn whether bytes remain, then stop.
      const next = await reader.read();
      if (!next.done && next.value && next.value.byteLength > 0) sawMore = true;
    }
  } finally {
    await reader.cancel().catch(() => {});
  }
  const merged = concatBytes(chunks).subarray(0, maxBytes);
  const truncated = declared !== undefined ? declared > maxBytes : sawMore;
  const totalBytes = declared ?? read;
  return { text: decoder.decode(merged), truncated, totalBytes };
}

function concatBytes(chunks: readonly Uint8Array[]): Uint8Array {
  if (chunks.length === 1) return chunks[0]!;
  const total = chunks.reduce((n, c) => n + c.byteLength, 0);
  const out = new Uint8Array(total);
  let offset = 0;
  for (const c of chunks) {
    out.set(c, offset);
    offset += c.byteLength;
  }
  return out;
}

function grepLines(text: string, pattern: string | RegExp): string {
  const test =
    typeof pattern === "string"
      ? (line: string) => line.toLowerCase().includes(pattern.toLowerCase())
      : (line: string) => pattern.test(line);
  return text
    .split("\n")
    .filter((line) => test(line))
    .join("\n");
}

/**
 * List a run's subagent CHILD runs (`GET /runs/:id/children`). Each row is a
 * {@link ChildRunRef} whose `id` resolves through the run facade (getRun /
 * events / outputs) — so every child the platform hands you is resolvable. An
 * empty array means the run spawned no children.
 */
export async function listRunChildren(
  http: HttpClient,
  runId: string
): Promise<readonly ChildRunRef[]> {
  const result = await http.request<
    { readonly children: readonly ChildRunRef[] } | readonly ChildRunRef[]
  >(`/api/runs/${encodeURIComponent(runId)}/children`);
  return Array.isArray(result)
    ? result
    : (result as { readonly children: readonly ChildRunRef[] }).children;
}

export async function cancelRun(http: HttpClient, runId: string): Promise<void> {
  await http.request<unknown>(
    `/api/runs/${encodeURIComponent(runId)}/cancel`,
    { method: "POST" }
  );
}

export async function deleteRun(http: HttpClient, runId: string): Promise<void> {
  await http.request<unknown>(
    `/api/runs/${encodeURIComponent(runId)}`,
    { method: "DELETE" }
  );
}

/**
 * List a run's webhook delivery attempts (the per-run delivery ledger). Returns
 * the rows surfaced by `GET /api/runs/:id/webhook-deliveries`; an empty array
 * means the run carried no `webhook` or has not reached a terminal state yet.
 */
export async function getRunWebhookDeliveries(
  http: HttpClient,
  runId: string
): Promise<readonly RunWebhookDelivery[]> {
  const result = await http.request<
    { readonly deliveries: readonly RunWebhookDelivery[] } | readonly RunWebhookDelivery[]
  >(`/api/runs/${encodeURIComponent(runId)}/webhook-deliveries`);
  return Array.isArray(result)
    ? result
    : (result as { readonly deliveries: readonly RunWebhookDelivery[] }).deliveries;
}

/**
 * Manually re-trigger a run's webhook delivery: resets the row to `pending` and
 * re-sends the frozen payload with the SAME `webhook-id` so the consumer
 * dedupes. Idempotent from the caller's view.
 */
export async function redeliverRunWebhook(
  http: HttpClient,
  runId: string,
  deliveryId: string
): Promise<void> {
  await http.request<unknown>(
    `/api/runs/${encodeURIComponent(runId)}/webhook-deliveries/${encodeURIComponent(deliveryId)}/redeliver`,
    { method: "POST" }
  );
}

/**
 * Delete a workspace asset cache entry. Accepts an `asset_<id>` value,
 * `sha256:<hex>`, or a bare 64-hex digest. Workspace is derived server-side
 * from the token; idempotent.
 * Does NOT affect runs that already snapshotted the asset.
 */
export async function deleteWorkspaceAsset(http: HttpClient, hash: string): Promise<void> {
  const assetId = hash.startsWith("asset_")
    ? hash
    : `asset_${hash.startsWith("sha256:") ? hash.slice("sha256:".length) : hash}`;
  await http.request<unknown>(`/assets/${encodeURIComponent(assetId)}`, { method: "DELETE" });
}

export async function whoami(http: HttpClient): Promise<WhoAmI> {
  return http.request<WhoAmI>("/api/whoami");
}

/**
 * Read the workspace billing summary (`GET /api/billing`, scope `billing:read`):
 * prepaid balance, current-month spend, spend cap, and plan fields. The result
 * is additive-tolerant — server fields this SDK does not know yet pass through.
 */
export async function getBilling(http: HttpClient): Promise<BillingSummary> {
  return http.request<BillingSummary>("/api/billing");
}

/**
 * Create a hosted checkout session for a paid plan. Returns only the hosted
 * URL; plan activation happens after checkout completes.
 */
export async function createBillingCheckout(
  http: HttpClient,
  request: BillingCheckoutRequest
): Promise<BillingHostedSession> {
  return http.request<BillingHostedSession>("/api/billing/checkout", {
    method: "POST",
    body: JSON.stringify(request)
  });
}

/**
 * Create a hosted billing-portal session for the workspace customer.
 * Returns only the hosted URL.
 */
export async function createBillingPortal(
  http: HttpClient,
  request: BillingPortalRequest = {}
): Promise<BillingHostedSession> {
  return http.request<BillingHostedSession>("/api/billing/portal", {
    method: "POST",
    body: JSON.stringify(request)
  });
}

/**
 * Read recent workspace credit-ledger rows (`GET /api/billing/ledger`, scope
 * `billing:read`), newest first. `limit` is clamped server-side to [1, 100]
 * (default 25); the read is not cursor-paged.
 */
export async function getBillingLedger(
  http: HttpClient,
  query?: BillingLedgerQuery
): Promise<BillingLedgerPage> {
  const params: Record<string, string> = {};
  if (query?.limit !== undefined) params.limit = String(query.limit);
  return http.request<BillingLedgerPage>("/api/billing/ledger", {}, params);
}

/**
 * Reveal the workspace webhook signing secret (`POST /api/webhook/signing-secret`),
 * CREATING one on first use. Repeat calls return the same `whsec_<base64>` value —
 * the hosted API does not rotate it. POST (not GET) so a reveal is a logged action.
 * Pass the returned `whsec` to `verifyAexWebhook` as `secret`.
 */
export async function getWebhookSigningSecret(http: HttpClient): Promise<WebhookSigningSecret> {
  return http.request<WebhookSigningSecret>("/api/webhook/signing-secret", { method: "POST" });
}

/**
 * A run's downloadable content is organised into three public namespaces, each
 * with a matching `download*` verb:
 *
 *   - `outputs`  — the run's real deliverables (`runs/<id>/outputs/`).
 *   - `events`   — typed event-channel records (`events.jsonl`).
 *   - `metadata` — the run record (`run.json`).
 *
 * `download` bundles all three as top-level folders; `downloadOutputs` /
 * `downloadEvents` / `downloadMetadata` each bundle one.
 * Every zip is assembled client-side from the public read endpoints —
 * there is no server-side archive route. Callers write the bytes to disk.
 */
type ArtifactNamespace = "outputs";

interface CollectedArtifacts {
  readonly entries: readonly ZipEntry[];
  readonly captured: RunRecordArtifactSummaryV1[];
  readonly errors: RunRecordDownloadErrorV1[];
}

interface ZipEntry extends RunRecordArchiveEntryForRedactionV1 {
  readonly path: string;
  readonly bytes: Uint8Array;
}

/**
 * Download each artifact's bytes into a zip-file map keyed by
 * `<zipPrefix><relative-path>`, fetched from the `outputs`
 * download route. Best-effort: a per-artifact fetch failure records an
 * `errors[]` entry rather than aborting the rest, so the failure is
 * surfaced (never silent) while a partially-available run still yields a
 * usable zip.
 */
async function collectArtifactBytes(
  http: HttpClient,
  runId: string,
  items: readonly Output[],
  zipPrefix: string,
  namespace: ArtifactNamespace
): Promise<CollectedArtifacts> {
  const entries: ZipEntry[] = [];
  const captured: RunRecordArtifactSummaryV1[] = [];
  const errors: RunRecordDownloadErrorV1[] = [];

  for (const item of items) {
    const rel = item.filename ?? item.id;
    try {
      const { response } = await http.download(
        `/api/runs/${encodeURIComponent(runId)}/${namespace}/${encodeURIComponent(item.id)}/download`
      );
      entries.push({
        path: `${zipPrefix}${rel}`,
        bytes: new Uint8Array(await response.arrayBuffer()),
        ...(item.contentType !== undefined ? { contentType: item.contentType } : {}),
        customerContent: true
      });
      captured.push({
        id: item.id,
        filename: item.filename ?? null,
        ...(item.sizeBytes !== undefined ? { sizeBytes: item.sizeBytes } : {}),
        ...(item.contentType !== undefined ? { contentType: item.contentType } : {})
      });
    } catch (err) {
      errors.push({ namespace, id: item.id, filename: item.filename ?? null, message: (err as Error).message });
    }
  }

  return { entries: Object.freeze(entries), captured, errors };
}

function eventsJsonl(events: readonly AexEvent[]): Uint8Array {
  return strToU8(events.map((event) => JSON.stringify(event)).join("\n"));
}

function isPathSelector(selector: OutputFileSelector): selector is OutputFilePathSelector {
  return Boolean(selector && typeof selector === "object" && "path" in selector);
}

function normalizeOutputLookupPath(path: string): string {
  return path.replace(/\\/g, "/").replace(/^\/+/, "");
}

export function filterOutputs(outputs: readonly Output[], query: OutputQuery): readonly Output[] {
  return outputs.filter((output) => outputMatchesQuery(output, query));
}

/**
 * The single filename-matcher for cross-run / per-session output SEARCH. A
 * string is a case-insensitive SUBSTRING match; a RegExp is tested as given (and
 * reset to `lastIndex = 0` so a reused `/g` regex is safe). Sharing this SSoT is
 * what closes the T16 crash class: `searchOutputs` no longer assumes `filename`
 * is a string and passes a RegExp into `escapeRegExp(...).replace(...)`.
 */
export function toFilenameMatcher(filename: string | RegExp): (name: string) => boolean {
  if (typeof filename === "string") {
    const needle = filename.toLowerCase();
    return (name: string) => name.toLowerCase().includes(needle);
  }
  return (name: string) => {
    filename.lastIndex = 0;
    return filename.test(name);
  };
}

export function classifyOutput(output: Pick<Output, "filename" | "contentType">): OutputFileType {
  const contentType = normalizeContentType(output.contentType);
  if (contentType) {
    if (contentType === "application/json" || contentType.endsWith("+json") || contentType.includes("json")) {
      return "json";
    }
    if (contentType.startsWith("text/")) return "text";
    if (contentType.startsWith("image/")) return "image";
    if (contentType.startsWith("audio/")) return "audio";
    if (contentType.startsWith("video/")) return "video";
    if (contentType === "application/pdf") return "pdf";
    if (
      contentType === "application/zip" ||
      contentType === "application/gzip" ||
      contentType === "application/x-gzip" ||
      contentType === "application/x-tar" ||
      contentType === "application/x-7z-compressed" ||
      contentType === "application/vnd.rar" ||
      contentType === "application/zstd"
    ) {
      return "archive";
    }
    if (contentType === "application/octet-stream") return "binary";
    return "unknown";
  }

  const extension = extensionOf(output.filename);
  if (!extension) return "unknown";
  if (["json", "jsonl", "ndjson"].includes(extension)) return "json";
  if (["txt", "log", "md", "markdown", "csv", "tsv", "xml", "html", "htm", "yaml", "yml"].includes(extension)) {
    return "text";
  }
  if (["png", "jpg", "jpeg", "gif", "webp", "avif", "bmp", "tif", "tiff", "svg"].includes(extension)) {
    return "image";
  }
  if (["mp3", "wav", "flac", "m4a", "aac", "ogg", "oga", "opus"].includes(extension)) return "audio";
  if (["mp4", "mov", "m4v", "webm", "mkv", "avi"].includes(extension)) return "video";
  if (extension === "pdf") return "pdf";
  if (["zip", "tar", "tgz", "gz", "bz2", "xz", "7z", "rar", "zst"].includes(extension)) return "archive";
  if (["bin", "exe", "dll", "so", "dylib", "dmg", "iso"].includes(extension)) return "binary";
  return "unknown";
}

export function normalizeOutputLinkExpiresIn(input: OutputLinkOptions["expiresIn"] = "1h"): number {
  if (typeof input === "number") {
    if (!Number.isFinite(input) || input <= 0) {
      throw new RunStateError("outputLink: expiresIn must be a positive number of seconds", {
        expiresIn: input
      });
    }
    return Math.floor(input);
  }
  if (input === "15m") return 15 * 60;
  if (input === "1h") return 60 * 60;
  if (input === "1d") return 24 * 60 * 60;
  throw new RunStateError("outputLink: expiresIn must be seconds, \"15m\", \"1h\", or \"1d\"", {
    expiresIn: input
  });
}

async function resolveOutputLinkTarget(
  http: HttpClient,
  runId: string,
  selectorOrQuery: OutputLinkSelector
): Promise<Output> {
  if (typeof selectorOrQuery === "string") {
    if (selectorOrQuery.length === 0) {
      throw new RunStateError("outputLink: selector must include an output id or query", { runId });
    }
    return { id: selectorOrQuery };
  }
  if (hasOutputId(selectorOrQuery)) {
    if (selectorOrQuery.id.length === 0) {
      throw new RunStateError("outputLink: selector must include an output id or query", { runId });
    }
    return selectorOrQuery;
  }
  if (isPathSelector(selectorOrQuery as OutputFileSelector) && (selectorOrQuery as OutputFilePathSelector).match === "suffix") {
    return resolveOutputFileSelector(await listOutputs(http, runId), selectorOrQuery as OutputFilePathSelector, runId);
  }
  const match = await findOutput(http, runId, selectorOrQuery as OutputQuery);
  if (match) return match;
  throw new RunStateError("outputLink: output query matched no files", { runId });
}

function outputMatchesQuery(output: Output, query: OutputQuery): boolean {
  const normalizedPath = typeof output.filename === "string" ? normalizeOutputQueryPath(output.filename) : "";
  if (query.path !== undefined && normalizedPath !== normalizeOutputQueryPath(query.path)) {
    return false;
  }
  if (query.filename !== undefined) {
    const basename = basenameOf(normalizedPath);
    if (typeof query.filename === "string") {
      if (basename !== query.filename) return false;
    } else {
      query.filename.lastIndex = 0;
      if (!query.filename.test(basename)) return false;
    }
  }
  if (query.dir !== undefined && !directoryMatches(normalizedPath, query.dir, query.recursive ?? true)) {
    return false;
  }
  if (query.extension !== undefined && extensionOf(normalizedPath) !== normalizeExtension(query.extension)) {
    return false;
  }
  if (query.contentType !== undefined && !contentTypeMatches(output.contentType, query.contentType)) {
    return false;
  }
  if (query.type !== undefined && classifyOutput(output) !== query.type) {
    return false;
  }
  return true;
}

function hasOutputId(value: OutputFileSelector | OutputQuery): value is Output {
  return Boolean(value && typeof value === "object" && "id" in value && typeof value.id === "string");
}

function normalizeOutputQueryPath(path: string): string {
  let normalized = path.replace(/\\/g, "/").replace(/^\/+/, "");
  while (normalized === "outputs" || normalized.startsWith("outputs/")) {
    normalized = normalized === "outputs" ? "" : normalized.slice("outputs/".length);
  }
  return normalized.replace(/\/+$/, "");
}

function basenameOf(path: string): string {
  return path.split("/").filter(Boolean).pop() ?? "";
}

function directoryMatches(path: string, dir: string, recursive: boolean): boolean {
  const normalizedDir = normalizeOutputQueryPath(dir);
  if (normalizedDir.length === 0) return true;
  const prefix = `${normalizedDir}/`;
  if (!path.startsWith(prefix)) return false;
  const remainder = path.slice(prefix.length);
  return remainder.length > 0 && (recursive || !remainder.includes("/"));
}

function normalizeExtension(extension: string): string {
  return extension.replace(/^\.+/, "").toLowerCase();
}

function extensionOf(path: string | undefined): string {
  if (!path) return "";
  const basename = basenameOf(normalizeOutputQueryPath(path));
  const index = basename.lastIndexOf(".");
  return index > 0 && index < basename.length - 1 ? basename.slice(index + 1).toLowerCase() : "";
}

function normalizeContentType(contentType: string | undefined): string {
  return (contentType ?? "").split(";")[0]!.trim().toLowerCase();
}

function contentTypeMatches(actual: string | undefined, expected: string): boolean {
  const normalizedActual = normalizeContentType(actual);
  const normalizedExpected = normalizeContentType(expected);
  if (!normalizedActual || !normalizedExpected) return false;
  if (normalizedExpected.endsWith("/*")) {
    return normalizedActual.startsWith(normalizedExpected.slice(0, -1));
  }
  return normalizedActual === normalizedExpected;
}

/**
 * Download EVERYTHING public about a run as one zip, organised into the three
 * namespace folders:
 *
 *   metadata/run.json     — the run record.
 *   events/events.jsonl   — typed event-channel records.
 *   outputs/<rel>         — the run's deliverables.
 *   manifest.json         — `RunRecordManifestV1`.
 */
export async function download(http: HttpClient, runId: string): Promise<Uint8Array> {
  const [run, events, outputs] = await Promise.all([
    getRun(http, runId),
    listRunEvents(http, runId),
    listOutputs(http, runId)
  ]);

  const out = await collectArtifactBytes(http, runId, outputs, "outputs/", "outputs");
  const submissionSnapshot = extractSubmissionSnapshot(run);
  const costTelemetry = extractCostTelemetry(run);
  const manifest = buildRunRecordDownloadManifestV1({
    runId,
    outputs: out.captured,
    errors: out.errors,
    typedEventCount: events.length,
    ...(submissionSnapshot ? { submission: { status: "present" } } : {}),
    ...(costTelemetry ? { cost: { status: "present" } } : {})
  });

  return zipEntries([
    jsonEntry("metadata/run.json", run),
    ...(submissionSnapshot ? [jsonEntry("metadata/submission.json", submissionSnapshot)] : []),
    ...(costTelemetry ? [jsonEntry("metadata/cost.json", costTelemetry)] : []),
    jsonlEntry("events/events.jsonl", events),
    ...out.entries,
    jsonEntry("manifest.json", manifest)
  ]);
}

/**
 * Download only the run's deliverables (the `outputs` namespace). Zip
 * layout: `<rel>` per file plus a `manifest.json`
 * (`{ runId, namespace: "outputs", outputs[], errors[] }`).
 */
export async function downloadOutputs(http: HttpClient, runId: string): Promise<Uint8Array> {
  const outputs = await listOutputs(http, runId);
  const { entries, captured, errors } = await collectArtifactBytes(http, runId, outputs, "", "outputs");
  return zipEntries([
    ...entries,
    jsonEntry("manifest.json", { runId, namespace: "outputs", outputs: captured, errors })
  ]);
}

/**
 * Download only the event archive (the `events` namespace). Always includes
 * typed `events.jsonl`.
 */
export async function downloadEvents(http: HttpClient, runId: string): Promise<Uint8Array> {
  const events = await listRunEvents(http, runId);
  return zipEntries([jsonlEntry("events.jsonl", events)]);
}

/**
 * Download only the run record (the `metadata` namespace) as a zip
 * containing `run.json`.
 */
export async function downloadMetadata(http: HttpClient, runId: string): Promise<Uint8Array> {
  const run = await getRun(http, runId);
  return zipEntries([jsonEntry("run.json", run)]);
}

function zipEntries(entries: readonly ZipEntry[]): Uint8Array {
  assertRunRecordArchivePublicSafeV1(entries);
  const files: Record<string, Uint8Array> = {};
  for (const entry of entries) {
    files[entry.path] = entry.bytes;
  }
  return zipSync(files);
}

function jsonEntry(path: string, value: unknown): ZipEntry {
  return {
    path,
    bytes: strToU8(JSON.stringify(value, null, 2)),
    contentType: "application/json; charset=utf-8"
  };
}

function jsonlEntry(path: string, events: readonly AexEvent[]): ZipEntry {
  return {
    path,
    bytes: eventsJsonl(events),
    contentType: "application/jsonl; charset=utf-8"
  };
}

function extractSubmissionSnapshot(run: Run): { readonly submission: PlatformSubmission } | undefined {
  const raw = (run as { readonly submission?: unknown }).submission;
  if (!isRecord(raw) || raw.kind !== "submission" || !isRecord(raw.submission)) {
    return undefined;
  }
  return {
    submission: raw.submission as unknown as PlatformSubmission
  };
}

function extractCostTelemetry(run: Run): RunCostTelemetry | undefined {
  const raw = run.costTelemetry;
  return isRecord(raw) ? raw as RunCostTelemetry : undefined;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

// ===========================================================================
// Run submission operations (McpServer / run config composition)
// ===========================================================================

export async function submitRun(
  http: HttpClient,
  request: PlatformRunSubmissionInput
): Promise<Run> {
  return http.request<Run>("/api/runs", {
    method: "POST",
    body: JSON.stringify(request)
  });
}

// ===========================================================================
// AgentsMd read/delete helpers. Launch submissions use content-addressed asset refs.
// ===========================================================================

export async function listAgentsMd(http: HttpClient): Promise<readonly AgentsMdRecord[]> {
  const result = await http.request<
    { readonly agentsMd: readonly AgentsMdRecord[] } | readonly AgentsMdRecord[]
  >("/api/agentsmd");
  if (Array.isArray(result)) {
    return result;
  }
  return (result as { readonly agentsMd: readonly AgentsMdRecord[] }).agentsMd;
}

export async function getAgentsMd(http: HttpClient, agentsMdId: string): Promise<AgentsMdRecord> {
  const result = await http.request<{ readonly agentsMd: AgentsMdRecord } | AgentsMdRecord>(
    `/api/agentsmd/${encodeURIComponent(agentsMdId)}`
  );
  return unwrapAgentsMd(result);
}

export async function deleteAgentsMd(http: HttpClient, agentsMdId: string): Promise<void> {
  await http.request<unknown>(`/api/agentsmd/${encodeURIComponent(agentsMdId)}`, {
    method: "DELETE"
  });
}

function unwrapAgentsMd(
  result: { readonly agentsMd: AgentsMdRecord } | AgentsMdRecord
): AgentsMdRecord {
  if (result && typeof result === "object" && "agentsMd" in (result as object)) {
    return (result as { readonly agentsMd: AgentsMdRecord }).agentsMd;
  }
  return result as AgentsMdRecord;
}

// ===========================================================================
// File read/delete helpers. Launch submissions use content-addressed asset refs.
// ===========================================================================

export async function listFiles(http: HttpClient): Promise<readonly FileRecord[]> {
  const result = await http.request<{ readonly files: readonly FileRecord[] } | readonly FileRecord[]>(
    "/api/files"
  );
  if (Array.isArray(result)) {
    return result;
  }
  return (result as { readonly files: readonly FileRecord[] }).files;
}

export async function getFile(http: HttpClient, fileId: string): Promise<FileRecord> {
  const result = await http.request<{ readonly file: FileRecord } | FileRecord>(
    `/api/files/${encodeURIComponent(fileId)}`
  );
  return unwrapFile(result);
}

export async function deleteFile(http: HttpClient, fileId: string): Promise<void> {
  await http.request<unknown>(`/api/files/${encodeURIComponent(fileId)}`, {
    method: "DELETE"
  });
}

function unwrapFile(result: { readonly file: FileRecord } | FileRecord): FileRecord {
  if (result && typeof result === "object" && "file" in (result as object)) {
    return (result as { readonly file: FileRecord }).file;
  }
  return result as FileRecord;
}

// ===========================================================================
// Workspace secret operations
//
// Value-bearing requests (create/rotate) carry the value in the JSON BODY,
// never the URL/query, so it never lands in logs or the request line. Reads
// split by sensitivity: `getSecret`/`listSecrets` return METADATA only;
// `getSecretValue` is the audited value read (POST so it's a logged action).
// ===========================================================================

/** Create a named workspace secret. The value travels in the body. */
export async function createSecret(
  http: HttpClient,
  args: { readonly name: string; readonly value: string }
): Promise<SecretRecord> {
  const result = await http.request<{ readonly secret: SecretRecord } | SecretRecord>("/api/secrets", {
    method: "POST",
    body: JSON.stringify({ name: args.name, value: args.value })
  });
  return unwrapSecret(result);
}

export async function listSecrets(http: HttpClient): Promise<readonly SecretRecord[]> {
  const result = await http.request<
    { readonly secrets: readonly SecretRecord[] } | readonly SecretRecord[]
  >("/api/secrets");
  if (Array.isArray(result)) {
    return result;
  }
  return (result as { readonly secrets: readonly SecretRecord[] }).secrets;
}

/** Metadata for one workspace secret by name. Never returns the value. */
export async function getSecret(http: HttpClient, name: string): Promise<SecretRecord> {
  const result = await http.request<{ readonly secret: SecretRecord } | SecretRecord>(
    `/api/secrets/${encodeURIComponent(name)}`
  );
  return unwrapSecret(result);
}

/** Audited value read — the preferred path that returns a workspace secret value. */
export async function getSecretValue(http: HttpClient, name: string): Promise<SecretReveal> {
  return http.request<SecretReveal>(`/api/secrets/${encodeURIComponent(name)}/get_value`, {
    method: "POST"
  });
}

/** Replace the value of an existing workspace secret; bumps its version. */
export async function rotateSecret(
  http: HttpClient,
  args: { readonly name: string; readonly value: string }
): Promise<SecretRecord> {
  const result = await http.request<{ readonly secret: SecretRecord } | SecretRecord>(
    `/api/secrets/${encodeURIComponent(args.name)}/rotate`,
    { method: "POST", body: JSON.stringify({ value: args.value }) }
  );
  return unwrapSecret(result);
}

export async function deleteSecret(http: HttpClient, name: string): Promise<void> {
  await http.request<unknown>(`/api/secrets/${encodeURIComponent(name)}`, {
    method: "DELETE"
  });
}

function unwrapSecret(result: { readonly secret: SecretRecord } | SecretRecord): SecretRecord {
  if (result && typeof result === "object" && "secret" in (result as object)) {
    return (result as { readonly secret: SecretRecord }).secret;
  }
  return result as SecretRecord;
}

// ===========================================================================
// Workspace skill registry operations
//
// Skills are named, mutable, by-name-bound bundles. `upsertSkill` UPSERTS one by
// name (the bytes are staged to the content-addressed asset store BEFORE this,
// via the presign/finalize path); the server compares `contentHash` and no-ops
// an identical re-upload (`updated:false`). Reads return METADATA only — the
// bytes are addressed by `contentHash`.
// ===========================================================================

/** Result of an `upsertSkill`: the stored record + whether the bytes changed. */
export interface SkillUpsertResult {
  readonly skill: SkillRecord;
  readonly updated: boolean;
}

/**
 * Upsert a workspace skill by name — `PUT /skills/{name}`. The bundle bytes must
 * already exist in the asset store (staged via presign/finalize before this
 * call); the body carries only the metadata. Identical `contentHash` is a no-op
 * (`updated:false`).
 */
export async function upsertSkill(
  http: HttpClient,
  args: { readonly name: string; readonly contentHash: string; readonly description: string; readonly sizeBytes: number }
): Promise<SkillUpsertResult> {
  const result = await http.request<SkillUpsertResult | SkillRecord>(
    `/api/skills/${encodeURIComponent(args.name)}`,
    {
      method: "PUT",
      body: JSON.stringify({
        contentHash: args.contentHash,
        description: args.description,
        sizeBytes: args.sizeBytes
      })
    }
  );
  if (result && typeof result === "object" && "skill" in (result as object)) {
    const wrapped = result as SkillUpsertResult;
    return { skill: wrapped.skill, updated: wrapped.updated === true };
  }
  return { skill: result as SkillRecord, updated: true };
}

export async function listSkills(http: HttpClient): Promise<readonly SkillRecord[]> {
  const result = await http.request<{ readonly skills: readonly SkillRecord[] } | readonly SkillRecord[]>(
    "/api/skills"
  );
  if (Array.isArray(result)) {
    return result;
  }
  return (result as { readonly skills: readonly SkillRecord[] }).skills;
}

export async function getSkill(http: HttpClient, name: string): Promise<SkillRecord> {
  const result = await http.request<{ readonly skill: SkillRecord } | SkillRecord>(
    `/api/skills/${encodeURIComponent(name)}`
  );
  return unwrapSkill(result);
}

export async function deleteSkill(http: HttpClient, name: string): Promise<void> {
  await http.request<unknown>(`/api/skills/${encodeURIComponent(name)}`, {
    method: "DELETE"
  });
}

function unwrapSkill(result: { readonly skill: SkillRecord } | SkillRecord): SkillRecord {
  if (result && typeof result === "object" && "skill" in (result as object)) {
    return (result as { readonly skill: SkillRecord }).skill;
  }
  return result as SkillRecord;
}

function hasRun(value: Run | { readonly run: Run }): value is { readonly run: Run } {
  return Boolean(value && typeof value === "object" && "run" in value);
}

function unwrapSession(result: { readonly session: Session } | Session): Session {
  if (result && typeof result === "object" && "session" in result) {
    return (result as { readonly session: Session }).session;
  }
  return result as Session;
}

// ===========================================================================
// Workspace asset upload
// ===========================================================================

export interface AssetUploadResult {
  readonly assetId: string;
  readonly contentHash: string;
  readonly sizeBytes: number;
  readonly exists: boolean;
}

/**
 * Upload bytes to the hosted API's content-addressable asset endpoint.
 * Returns a storage-neutral asset id suitable for `kind:"asset"` refs in a
 * later run submission.
 */
export async function uploadWorkspaceAsset(
  http: HttpClient,
  input: {
    readonly bytes: Uint8Array;
    /** Optional `sha256:<hex>` advisory hash; the server verifies it. */
    readonly contentHash?: string;
    readonly contentType?: string;
  }
): Promise<AssetUploadResult> {
  return http.request<AssetUploadResult>("/assets", {
    method: "POST",
    headers: {
      "content-type": input.contentType ?? "application/octet-stream",
      "content-length": String(input.bytes.byteLength),
      ...(input.contentHash ? { "x-asset-hash": input.contentHash } : {})
    },
    body: input.bytes
  });
}
