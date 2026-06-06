import { strToU8, zipSync } from "fflate";
import type { HttpClient } from "./http.js";
import type { RunUnit } from "./run-unit.js";
import { RunStateError } from "./sdk-errors.js";
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
  FileRecord,
  Output,
  OutputFileDownload,
  OutputFilePathSelector,
  OutputFileSelector,
  Run,
  RunEvent,
  SignedOutputLink,
  Skill,
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
 * Workspace identity is derived server-side from the API token on
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
 * outputs, capture failures, proxy-call audit, pinned skills,
 * provider skills, inline skills.
 *
 * Backed by the same `GET /api/runs/:runId` endpoint that
 * `getRun` calls; this variant just narrows the return type to
 * the documented wire shape. Prefer this for new code; `getRun`
 * stays for callers that only need the loose record.
 */
export async function getRunUnit(http: HttpClient, runId: string): Promise<RunUnit> {
  return http.request<RunUnit>(`/api/runs/${encodeURIComponent(runId)}`);
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
): Promise<readonly RunEvent[]> {
  const path = `/api/runs/${encodeURIComponent(runId)}/events`;
  const all: RunEvent[] = [];
  let cursor: number | undefined;
  for (let page = 0; page < LIST_EVENTS_PAGE_BUDGET; page++) {
    const query = cursor !== undefined ? { cursor: String(cursor) } : {};
    const result = await http.request<{ readonly events: readonly RunEvent[]; readonly nextCursor?: number | null }>(
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
  runId: string
): Promise<readonly Output[]> {
  const result = await http.request<{ readonly outputs: readonly Output[] }>(
    `/api/runs/${encodeURIComponent(runId)}/outputs`
  );
  return result.outputs;
}

export async function createOutputLink(
  http: HttpClient,
  runId: string,
  outputId: string
): Promise<SignedOutputLink> {
  return http.request<SignedOutputLink>(
    `/api/runs/${encodeURIComponent(runId)}/outputs/${encodeURIComponent(outputId)}/link`,
    { method: "POST" }
  );
}

export function resolveOutputFileSelector(
  outputs: readonly Output[],
  selector: OutputFileSelector,
  runId?: string
): Output {
  if (isPathSelector(selector)) {
    const target = normalizeOutputLookupPath(selector.path);
    if (!target) {
      throw new RunStateError("downloadOutput: output path must be non-empty", { runId, path: selector.path });
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
        `downloadOutput: output path "${selector.path}" matched multiple files`,
        { runId, path: selector.path, matches: matches.map((output) => output.filename ?? output.id) }
      );
    }
    throw new RunStateError(`downloadOutput: output path "${selector.path}" was not found`, {
      runId,
      path: selector.path
    });
  }
  if (typeof selector?.id !== "string" || selector.id.length === 0) {
    throw new RunStateError("downloadOutput: selector must include an output id or path", { runId });
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

function eventsJsonl(events: readonly RunEvent[]): Uint8Array {
  return strToU8(events.map((event) => JSON.stringify(event)).join("\n"));
}

function isPathSelector(selector: OutputFileSelector): selector is OutputFilePathSelector {
  return Boolean(selector && typeof selector === "object" && "path" in selector);
}

function normalizeOutputLookupPath(path: string): string {
  return path.replace(/\\/g, "/").replace(/^\/+/, "");
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

function jsonlEntry(path: string, events: readonly RunEvent[]): ZipEntry {
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
// Run submission operations (Skill / McpServer / run config composition)
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

/**
 * Multipart variant of `submitRun` for runs that carry transient
 * (per-run) skill bundles and/or transient AgentsMd content.
 *
 * The JSON submission travels as the `submission` part; each
 * `InlineSkillRef.slot` in `request.submission.skills` MUST be
 * mirrored by exactly one `skill:<slot>` part with the bundle bytes.
 * Each `InlineAgentsMdRef.slot` in `request.submission.agentsMd`
 * MUST be mirrored by exactly one `agentsmd:<slot>` part with the
 * markdown text.
 *
 * The BFF re-canonicalises and re-hashes each bundle/file; a
 * `contentHash` mismatch is rejected with a deterministic error.
 *
 * At least one of `bundles` or `agentsMdParts` must be non-empty.
 */
export async function submitRunMultipart(
  http: HttpClient,
  request: PlatformRunSubmissionInput,
  bundles: ReadonlyArray<{
    readonly slot: string;
    readonly bytes: Uint8Array;
    readonly filename: string;
  }>,
  agentsMdParts?: ReadonlyArray<{
    readonly slot: string;
    readonly content: string;
    readonly filename: string;
  }>,
  fileParts?: ReadonlyArray<{
    readonly slot: string;
    readonly bytes: Uint8Array;
    readonly filename: string;
  }>
): Promise<Run> {
  const hasBundles = Array.isArray(bundles) && bundles.length > 0;
  const hasAgentsMd = Array.isArray(agentsMdParts) && agentsMdParts.length > 0;
  const hasFiles = Array.isArray(fileParts) && fileParts.length > 0;
  if (!hasBundles && !hasAgentsMd && !hasFiles) {
    throw new Error("submitRunMultipart: bundles, agentsMdParts, or fileParts must be non-empty");
  }
  const form = new FormData();
  // Submission rides as a typed JSON Blob so the BFF reads
  // `multipart["submission"]` with the right content-type and never
  // has to re-detect the body shape.
  form.append(
    "submission",
    new Blob([JSON.stringify(request)], { type: "application/json" }),
    "submission.json"
  );
  const seen = new Set<string>();
  for (const bundle of bundles) {
    if (typeof bundle.slot !== "string" || !bundle.slot) {
      throw new Error("submitRunMultipart: each bundle must have a non-empty slot id");
    }
    if (seen.has(bundle.slot)) {
      throw new Error(`submitRunMultipart: duplicate inline skill slot "${bundle.slot}"`);
    }
    seen.add(bundle.slot);
    const blob = toBlob(bundle.bytes, "application/zip");
    form.append(`skill:${bundle.slot}`, blob, bundle.filename);
  }
  for (const part of agentsMdParts ?? []) {
    if (typeof part.slot !== "string" || !part.slot) {
      throw new Error("submitRunMultipart: each agentsMd part must have a non-empty slot id");
    }
    const partKey = `agentsmd:${part.slot}`;
    if (seen.has(partKey)) {
      throw new Error(`submitRunMultipart: duplicate agentsMd slot "${part.slot}"`);
    }
    seen.add(partKey);
    const blob = new Blob([part.content], { type: "text/plain" });
    form.append(partKey, blob, part.filename);
  }
  for (const part of fileParts ?? []) {
    if (typeof part.slot !== "string" || !part.slot) {
      throw new Error("submitRunMultipart: each file part must have a non-empty slot id");
    }
    const partKey = `file:${part.slot}`;
    if (seen.has(partKey)) {
      throw new Error(`submitRunMultipart: duplicate file slot "${part.slot}"`);
    }
    seen.add(partKey);
    const blob = toBlob(part.bytes, "application/zip");
    form.append(partKey, blob, part.filename);
  }
  return http.request<Run>("/api/runs", {
    method: "POST",
    body: form
  });
}

/**
 * Upload a workspace skill bundle as a zip blob. The hosted API runs
 * the two-phase flow internally (insert pending row, stream bytes into
 * object storage, validate manifest, transition to ready) and returns
 * the finalized `Skill`. Use `Skill.fromPath` / `Skill.upload` in the
 * SDK to build the body; this transport function only knows about
 * bytes.
 */
export async function createSkillBundle(
  http: HttpClient,
  args: {
    readonly name: string;
    readonly body: Blob | ArrayBuffer | Uint8Array;
    readonly contentType?: string;
    readonly filename?: string;
  }
): Promise<Skill> {
  const form = new FormData();
  form.append("name", args.name);
  const blobBody = toBlob(args.body, args.contentType ?? "application/zip");
  form.append("bundle", blobBody, args.filename ?? `${args.name}.zip`);
  const result = await http.request<{ readonly skill: Skill } | Skill>("/api/skills", {
    method: "POST",
    body: form
  });
  return unwrapSkill(result);
}

/**
 * Upload a workspace skill bundle DIRECTLY to object storage via the presign
 * flow, so the bytes never transit the hosted API (bundle size bounded by the
 * object store, not API memory). Falls back to the buffered multipart
 * `createSkillBundle` when the hosted API has no object-store upload
 * credentials (503 `presign_unconfigured`).
 *
 *   1. POST /api/skills/presign     { name, hash, sizeBytes } → { uploadUrl, requiredHeaders, skillId }
 *   2. PUT bytes → uploadUrl        (signed checksum; the store rejects a mismatch)
 *   3. POST /api/skills/:id/finalize { manifest } → finalized Skill
 *
 * `manifest` is the client-computed bundle manifest (the caller already
 * validated the zip shape before hashing); the Worker records it on finalize
 * without re-buffering the object.
 */
export async function createSkillBundleDirect(
  http: HttpClient,
  fetchImpl: (input: string, init?: RequestInit) => Promise<{ ok: boolean; status: number; text(): Promise<string> }>,
  args: {
    readonly name: string;
    readonly body: Uint8Array;
    readonly contentHash: string; // `sha256:<hex>`
    readonly manifest: ReadonlyArray<{ readonly path: string; readonly size: number; readonly mode?: number }>;
    readonly contentType?: string;
  }
): Promise<Skill> {
  let presign: {
    ok: boolean;
    skillId: string;
    uploadUrl: string;
    requiredHeaders?: Record<string, string>;
  };
  try {
    presign = await http.request<typeof presign>("/api/skills/presign", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ name: args.name, hash: args.contentHash, sizeBytes: args.body.byteLength })
    });
  } catch (err) {
    const status = (err as { status?: number }).status;
    const code = ((err as { details?: { code?: string } }).details ?? {}).code;
    if (status === 503 && code === "presign_unconfigured") {
      return createSkillBundle(http, {
        name: args.name,
        body: args.body,
        ...(args.contentType ? { contentType: args.contentType } : {})
      });
    }
    throw err;
  }

  const putRes = await fetchImpl(presign.uploadUrl, {
    method: "PUT",
    headers: { "content-type": args.contentType ?? "application/zip", ...(presign.requiredHeaders ?? {}) },
    body: args.body as unknown as BodyInit
  });
  if (!putRes.ok) {
    const detail = await putRes.text().catch(() => "");
    throw new Error(`createSkillBundleDirect: direct upload PUT failed (status ${putRes.status})${detail ? `: ${detail.slice(0, 500)}` : ""}`);
  }

  const result = await http.request<{ readonly skill: Skill } | Skill>(
    `/api/skills/${encodeURIComponent(presign.skillId)}/finalize`,
    {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ manifest: args.manifest })
    }
  );
  return unwrapSkill(result);
}

export async function listSkills(http: HttpClient): Promise<readonly Skill[]> {
  const result = await http.request<{ readonly skills: readonly Skill[] } | readonly Skill[]>(
    "/api/skills"
  );
  if (Array.isArray(result)) {
    return result;
  }
  return (result as { readonly skills: readonly Skill[] }).skills;
}

export async function getSkill(http: HttpClient, skillId: string): Promise<Skill> {
  const result = await http.request<{ readonly skill: Skill } | Skill>(
    `/api/skills/${encodeURIComponent(skillId)}`
  );
  return unwrapSkill(result);
}

export async function deleteSkill(http: HttpClient, skillId: string): Promise<void> {
  await http.request<unknown>(`/api/skills/${encodeURIComponent(skillId)}`, {
    method: "DELETE"
  });
}

/**
 * Lookup a live workspace skill by `(name, contentHash)`. Returns the
 * matching `Skill` record or null when no live row carries that hash.
 *
 * `contentHash` is the wire format `sha256:<hex>` as returned by
 * `hashSkillBundle`. This powers `Skill.uploadIfChanged` — the SDK
 * computes the hash locally and calls this function to skip the upload
 * when the bytes already exist.
 */
export async function findSkillByHash(
  http: HttpClient,
  args: { readonly name: string; readonly contentHash: string }
): Promise<Skill | null> {
  const params = new URLSearchParams({
    name: args.name,
    content_hash: args.contentHash
  });
  const result = await http.request<{ readonly skill: Skill | null }>(
    `/api/skills/by-hash?${params.toString()}`
  );
  return result.skill ?? null;
}

/**
 * Lookup a live workspace skill by `name`. Returns the matching `Skill`
 * record or null when no live row carries that name. Implemented as a
 * list-and-filter on the existing `/api/skills` endpoint — the
 * indexed by-hash route is reserved for `uploadIfChanged`.
 */
export async function findSkillByName(http: HttpClient, name: string): Promise<Skill | null> {
  const skills = await listSkills(http);
  return skills.find((skill) => skill.name === name) ?? null;
}

// ===========================================================================
// AgentsMd (workspace_files kind='agentsmd') operations
// ===========================================================================

/**
 * Upload a workspace AgentsMd file as a markdown string. The BFF
 * canonicalises the content into a deterministic zip with AGENTS.md at
 * root and runs the two-phase pending → ready upload.
 */
export async function createAgentsMd(
  http: HttpClient,
  args: {
    readonly name: string;
    readonly content: string;
  }
): Promise<AgentsMdRecord> {
  const form = new FormData();
  form.append("name", args.name);
  form.append(
    "content",
    new Blob([args.content], { type: "text/plain" }),
    "AGENTS.md"
  );
  const result = await http.request<{ readonly agentsMd: AgentsMdRecord } | AgentsMdRecord>(
    "/api/agentsmd",
    { method: "POST", body: form }
  );
  return unwrapAgentsMd(result);
}

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
// File (workspace_files kind='file') operations
// ===========================================================================

/**
 * Upload a workspace File as a zip bundle. The BFF canonicalises the
 * content and runs the two-phase pending → ready upload.
 */
export async function createFile(
  http: HttpClient,
  args: {
    readonly name: string;
    readonly bytes: Uint8Array;
  }
): Promise<FileRecord> {
  const form = new FormData();
  form.append("name", args.name);
  const blob = toBlob(args.bytes, "application/zip");
  form.append("bundle", blob, `${args.name}.zip`);
  const result = await http.request<{ readonly file: FileRecord } | FileRecord>(
    "/api/files",
    { method: "POST", body: form }
  );
  return unwrapFile(result);
}

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

function unwrapSkill(result: { readonly skill: Skill } | Skill): Skill {
  if (result && typeof result === "object" && "skill" in (result as object)) {
    return (result as { readonly skill: Skill }).skill;
  }
  return result as Skill;
}

function toBlob(input: Blob | ArrayBuffer | Uint8Array, contentType: string): Blob {
  if (input instanceof Blob) {
    return input;
  }
  if (input instanceof Uint8Array) {
    // BlobPart accepts ArrayBufferView, but lib.dom's overload set
    // narrows on the underlying buffer kind. Slice into a fresh
    // ArrayBuffer so a SharedArrayBuffer-backed Uint8Array works.
    const copy = new Uint8Array(input.byteLength);
    copy.set(input);
    return new Blob([copy.buffer], { type: contentType });
  }
  return new Blob([input], { type: contentType });
}

function hasRun(value: Run | { readonly run: Run }): value is { readonly run: Run } {
  return Boolean(value && typeof value === "object" && "run" in value);
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
