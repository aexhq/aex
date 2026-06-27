/**
 * `aex debug <runId>` — OPERATOR/admin command.
 *
 * Unlike the public host verbs (`status`, `events`, `outputs`, …) which talk
 * to the HTTP API with an `--api-token`, this command reads the AWS plane
 * directly and assembles ONE local debug bundle for a run from every durable
 * source:
 *
 *   1. S3 `runs/<runId>/` prefix (boot.json, settle.json, session/diag-*,
 *      session/brain-*.ndjson, an outputs/ listing) on the OUTPUTS bucket.
 *   2. The event journal — DDB `<prefix>-events` (pk=runId, asc by seq), with a
 *      fallback to the S3 `events-archive/<runId>/log.ndjson` cold log when DDB
 *      rows are empty/TTL-expired.
 *   3. SFN `GetExecutionHistory` for the execution named `runId`.
 *   4. CloudWatch (best-effort, `--cloudwatch`) across the api Lambda, SFN,
 *      brain Fargate, and egress log groups, filtered by runId + time window.
 *   5. Recursion into child/subagent runs (separate runIds) → nested bundles.
 *
 * Resilience contract: a missing/expired source is a NOTED gap in the bundle
 * manifest/index.md — never a hard failure. Only "can't resolve the plane at
 * all" (no creds and no `--account`) or "can't write the bundle" fail.
 *
 * Credentials: the standard AWS SDK v3 chain (env vars / shared profile),
 * exactly as an operator runs it. The AWS SDK clients are loaded lazily via a
 * runtime `import()` of a non-literal specifier so the default customer CLI
 * bundle never pays for (or ships) the SDK — only `aex debug` pulls it in.
 *
 * Resource naming mirrors terraform `modules/region`:
 *   prefix          = `aex-<plane>-<region>`
 *   outputs bucket  = `<prefix>-outputs-<account>`
 *   archive bucket  = `<prefix>-events-archive-<account>`
 *   events table    = `<prefix>-events`
 *   state machine   = `<prefix>-sfn`
 *   log groups      = `/aws/lambda/<prefix>-api`, `/aws/states/<prefix>-run`,
 *                     `/aws/ecs/<prefix>-brain`, `/aws/ecs/<prefix>-egress`
 */
import { dirname, resolve as resolvePath } from "node:path";
import type { CliIO } from "../internal.js";
import {
  type CliExitCode,
  RUNTIME_ERR,
  SUCCESS,
  USAGE_ERR,
  parseDuration,
  refuseInsideManagedRun,
  takeBooleanFlag,
  takeOptionFlag
} from "./common.js";

// ---------------------------------------------------------------------------
// Public types (also imported by the unit test, which injects a fake
// DebugFetchers so no live AWS is required).
// ---------------------------------------------------------------------------

export interface S3ListItem {
  readonly key: string;
  readonly sizeBytes: number;
}

/** A single journal record (DDB item or archived ndjson line), unmarshalled. */
export type JournalEvent = Record<string, unknown>;

export type LogFetchResult =
  | { readonly kind: "events"; readonly events: readonly unknown[] }
  | { readonly kind: "empty" }
  | { readonly kind: "expired" }
  | { readonly kind: "error"; readonly message: string };

/** The read surfaces the bundle assembler depends on (mocked in tests). */
export interface DebugFetchers {
  listObjects(bucket: string, prefix: string): Promise<readonly S3ListItem[]>;
  /** Object body as text, or null when the object is absent/unreadable. */
  getObjectText(bucket: string, key: string): Promise<string | null>;
  /** DDB events query (pk=runId, asc by seq). null = not found / error. */
  queryEvents(table: string, runId: string): Promise<readonly JournalEvent[] | null>;
  /** SFN GetExecutionHistory by execution ARN. null = not found / error. */
  getExecutionHistory(executionArn: string): Promise<readonly unknown[] | null>;
  /** CloudWatch FilterLogEvents in [startMs,endMs], filtered by runId. */
  filterLogs(logGroup: string, runId: string, startMs: number, endMs: number): Promise<LogFetchResult>;
}

export interface DebugTargets {
  readonly plane: string;
  readonly region: string;
  readonly partition: string;
  readonly account: string | null;
  readonly outputsBucket: string;
  readonly eventsArchiveBucket: string;
  readonly eventsTable: string;
  readonly stateMachineName: string;
  readonly logGroups: {
    readonly api: string;
    readonly sfn: string;
    readonly brain: string;
    readonly egress: string;
  };
}

/** AWS-backed client = the fetchers + plane target resolution. */
export interface AwsDebugClient extends DebugFetchers {
  resolveTargets(plane: string, region: string, accountHint: string | null): Promise<DebugTargets>;
}

type SourceState = "present" | "empty" | "expired" | "missing" | "error" | "skipped";

export interface SourceStatus {
  readonly source: string;
  readonly state: SourceState;
  readonly detail?: string;
  readonly count?: number;
}

export interface RunManifest {
  readonly runId: string;
  readonly generatedAt: string;
  readonly plane: string;
  readonly region: string;
  readonly outputsBucket: string;
  readonly eventsArchiveBucket: string;
  readonly runStatus: string | null;
  readonly startedAt: string | null;
  readonly endedAt: string | null;
  readonly journalSource: "ddb" | "events-archive" | "none";
  readonly journalEventCount: number;
  readonly childRunIds: readonly string[];
  readonly sources: readonly SourceStatus[];
  readonly children: readonly RunManifest[];
}

export interface BundleFile {
  /** Relative path within the bundle root, forward-slash separated. */
  readonly path: string;
  readonly content: string;
}

export interface RunBundle {
  readonly files: readonly BundleFile[];
  readonly manifest: RunManifest;
}

interface AssembleFlags {
  readonly cloudwatch: boolean;
  readonly withOutputs: boolean;
  readonly sinceMs: number;
}

// ---------------------------------------------------------------------------
// Pure helpers (exported for unit testing).
// ---------------------------------------------------------------------------

function status(source: string, state: SourceState, opts: { detail?: string; count?: number } = {}): SourceStatus {
  return {
    source,
    state,
    ...(opts.detail !== undefined ? { detail: opts.detail } : {}),
    ...(opts.count !== undefined ? { count: opts.count } : {})
  };
}

function seqOf(v: unknown): number {
  if (typeof v === "number" && Number.isFinite(v)) return v;
  if (typeof v === "string" && /^\d+$/.test(v)) return Number(v);
  return Number.POSITIVE_INFINITY;
}

/** Stable sort journal events by numeric `seq` ascending (seqless → tail). */
export function sortJournal(events: readonly JournalEvent[]): JournalEvent[] {
  return events
    .map((e, i) => ({ e, i }))
    .sort((a, b) => {
      const d = seqOf(a.e.seq) - seqOf(b.e.seq);
      return d !== 0 ? d : a.i - b.i;
    })
    .map((x) => x.e);
}

/** Parse ndjson text into journal events, skipping blank/malformed lines. */
export function parseNdjson(text: string): JournalEvent[] {
  const out: JournalEvent[] = [];
  for (const line of text.split("\n")) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    try {
      const parsed: unknown = JSON.parse(trimmed);
      if (parsed && typeof parsed === "object") out.push(parsed as JournalEvent);
    } catch {
      /* skip non-JSON line */
    }
  }
  return out;
}

const CHILD_ID_KEYS = ["childRunId", "subagentRunId", "spawnedRunId", "child_run_id", "subagent_run_id"];

/**
 * Find child/subagent run ids referenced anywhere in the journal. Scans
 * recursively for the well-known lineage keys; excludes the run's own id.
 */
export function extractChildRunIds(events: readonly JournalEvent[], selfRunId: string): string[] {
  const ids = new Set<string>();
  const scan = (node: unknown): void => {
    if (!node || typeof node !== "object") return;
    if (Array.isArray(node)) {
      for (const v of node) scan(v);
      return;
    }
    const rec = node as Record<string, unknown>;
    for (const k of CHILD_ID_KEYS) {
      const v = rec[k];
      if (typeof v === "string" && v && v !== selfRunId) ids.add(v);
    }
    for (const v of Object.values(rec)) scan(v);
  };
  for (const e of events) scan(e);
  return [...ids];
}

function parseJsonObj(text: string | null): Record<string, unknown> {
  if (!text) return {};
  try {
    const parsed: unknown = JSON.parse(text);
    return parsed && typeof parsed === "object" && !Array.isArray(parsed) ? (parsed as Record<string, unknown>) : {};
  } catch {
    return {};
  }
}

function pickStr(obj: Record<string, unknown>, keys: readonly string[]): string | null {
  for (const k of keys) {
    const v = obj[k];
    if (typeof v === "string" && v) return v;
  }
  return null;
}

function pickTime(obj: Record<string, unknown>, keys: readonly string[]): string | null {
  for (const k of keys) {
    const v = obj[k];
    if (typeof v === "string" && v) return v;
    if (typeof v === "number" && Number.isFinite(v)) return new Date(v).toISOString();
  }
  return null;
}

export function extractRunMeta(
  settleText: string | null,
  bootText: string | null
): { status: string | null; startedAt: string | null; endedAt: string | null } {
  const settle = parseJsonObj(settleText);
  const boot = parseJsonObj(bootText);
  return {
    status: pickStr(settle, ["status", "runStatus", "outcome", "state", "terminalStatus"]) ?? pickStr(boot, ["status"]),
    startedAt:
      pickTime(boot, ["startedAt", "createdAt", "bootAt", "queuedAt"]) ??
      pickTime(settle, ["startedAt", "createdAt"]),
    endedAt: pickTime(settle, ["endedAt", "finishedAt", "terminalAt", "settledAt", "completedAt", "ts"])
  };
}

export function renderIndexMd(m: RunManifest, depth = 0): string {
  const lines: string[] = [];
  const h = "#".repeat(Math.min(depth + 1, 6));
  const sub = "#".repeat(Math.min(depth + 2, 6));
  lines.push(`${h} aex debug bundle — ${m.runId}`, "");
  lines.push(`- generated: ${m.generatedAt}`);
  lines.push(`- plane / region: ${m.plane} / ${m.region}`);
  lines.push(`- run status: ${m.runStatus ?? "unknown"}`);
  lines.push(`- started: ${m.startedAt ?? "?"}   ended: ${m.endedAt ?? "?"}`);
  lines.push(`- journal: ${m.journalSource} (${m.journalEventCount} events)`);
  lines.push(`- outputs bucket: ${m.outputsBucket}`, "");
  lines.push(`${sub} sources`, "");
  lines.push("| source | state | detail | count |");
  lines.push("|---|---|---|---|");
  for (const s of m.sources) {
    lines.push(`| ${s.source} | ${s.state} | ${(s.detail ?? "").replace(/\|/g, "\\|")} | ${s.count ?? ""} |`);
  }
  lines.push("");
  if (m.childRunIds.length > 0) {
    lines.push(`${sub} child runs (${m.childRunIds.length})`, "");
    for (const c of m.childRunIds) lines.push(`- ${c} → children/${c}/`);
    lines.push("");
    for (const cm of m.children) lines.push(renderIndexMd(cm, depth + 1), "");
  }
  return lines.join("\n");
}

// ---------------------------------------------------------------------------
// Bundle assembly (source-agnostic; the AWS clients are injected as
// `DebugFetchers` so this is fully unit-testable with fakes).
// ---------------------------------------------------------------------------

export async function assembleBundle(opts: {
  readonly runId: string;
  readonly targets: DebugTargets;
  readonly sources: DebugFetchers;
  readonly cloudwatch: boolean;
  readonly withOutputs: boolean;
  readonly sinceMs: number;
  readonly visited?: Set<string>;
}): Promise<RunBundle> {
  const flags: AssembleFlags = { cloudwatch: opts.cloudwatch, withOutputs: opts.withOutputs, sinceMs: opts.sinceMs };
  return assembleOne(opts.runId, opts.targets, opts.sources, flags, opts.visited ?? new Set<string>());
}

async function safeGetText(sources: DebugFetchers, bucket: string, key: string): Promise<string | null> {
  try {
    return await sources.getObjectText(bucket, key);
  } catch {
    return null;
  }
}

async function assembleOne(
  runId: string,
  targets: DebugTargets,
  sources: DebugFetchers,
  flags: AssembleFlags,
  visited: Set<string>
): Promise<RunBundle> {
  visited.add(runId);
  const files: BundleFile[] = [];
  const statuses: SourceStatus[] = [];

  // --- 1. S3 runs/<runId>/ prefix --------------------------------------
  const prefix = `runs/${runId}/`;
  let listing: readonly S3ListItem[] = [];
  let s3Error: string | null = null;
  try {
    listing = await sources.listObjects(targets.outputsBucket, prefix);
  } catch (err) {
    s3Error = (err as Error).message ?? "list failed";
  }
  files.push({
    path: "s3-listing.ndjson",
    content: ndjsonOf(listing.map((i) => ({ key: i.key, sizeBytes: i.sizeBytes })))
  });

  const outputsListing: S3ListItem[] = [];
  let sealedPresent = false;
  let sealedSize = 0;
  let settleText: string | null = null;
  let bootText: string | null = null;

  for (const item of listing) {
    if (!item.key.startsWith(prefix)) continue;
    const rel = item.key.slice(prefix.length);
    if (rel === "") continue;
    if (rel === "secrets.sealed") {
      sealedPresent = true;
      sealedSize = item.sizeBytes;
      continue; // never download the sealed blob
    }
    if (rel.startsWith("outputs/")) {
      outputsListing.push(item);
      if (flags.withOutputs) {
        const body = await safeGetText(sources, targets.outputsBucket, item.key);
        if (body !== null) files.push({ path: rel, content: body });
      }
      continue;
    }
    // assets/* (may be large/binary) and session/backup/* are listed only.
    if (rel.startsWith("assets/") || rel.startsWith("session/backup/")) continue;
    if (/\.(json|ndjson|txt|log)$/.test(rel)) {
      const body = await safeGetText(sources, targets.outputsBucket, item.key);
      if (body !== null) {
        files.push({ path: rel, content: body });
        if (rel === "settle.json") settleText = body;
        if (rel === "boot.json" || rel === "session/boot.json") bootText = body;
      }
    }
  }
  files.push({
    path: "outputs/_listing.ndjson",
    content: ndjsonOf(outputsListing.map((i) => ({ key: i.key, sizeBytes: i.sizeBytes })))
  });

  if (s3Error) statuses.push(status("s3", "error", { detail: s3Error }));
  else if (listing.length === 0) {
    statuses.push(status("s3", "missing", { detail: `no objects under s3://${targets.outputsBucket}/${prefix}` }));
  } else statuses.push(status("s3", "present", { count: listing.length }));
  statuses.push(
    sealedPresent
      ? status("secrets.sealed", "present", { detail: `${sealedSize} bytes (not downloaded)` })
      : status("secrets.sealed", "missing", { detail: "absent" })
  );

  const meta = extractRunMeta(settleText, bootText);

  // --- 2. Event journal (DDB → events-archive fallback) -----------------
  let journalSource: RunManifest["journalSource"] = "none";
  let events: JournalEvent[] = [];
  let ddbRows: readonly JournalEvent[] | null = null;
  let ddbError: string | null = null;
  try {
    ddbRows = await sources.queryEvents(targets.eventsTable, runId);
  } catch (err) {
    ddbError = (err as Error).message ?? "query failed";
  }
  if (ddbRows && ddbRows.length > 0) {
    events = sortJournal(ddbRows);
    journalSource = "ddb";
  } else {
    const archiveText = await safeGetText(sources, targets.eventsArchiveBucket, `${runId}/log.ndjson`);
    if (archiveText) {
      events = sortJournal(parseNdjson(archiveText));
      if (events.length > 0) journalSource = "events-archive";
    }
  }
  files.push({ path: "journal.ndjson", content: ndjsonOf(events) });
  if (journalSource === "none") {
    statuses.push(
      status("journal", "missing", {
        detail: ddbError
          ? `ddb error: ${ddbError}; no events-archive fallback`
          : "empty in DDB and no events-archive/<runId>/log.ndjson (TTL-expired?)"
      })
    );
  } else {
    statuses.push(status("journal", "present", { detail: `source=${journalSource}`, count: events.length }));
  }

  // --- 3. SFN execution history ----------------------------------------
  if (!targets.account) {
    statuses.push(
      status("sfn", "skipped", { detail: "no account id (pass --account or grant s3:ListBuckets) — cannot build execution ARN" })
    );
  } else {
    const arn = `arn:${targets.partition}:states:${targets.region}:${targets.account}:execution:${targets.stateMachineName}:${runId}`;
    let history: readonly unknown[] | null = null;
    let sfnError: string | null = null;
    try {
      history = await sources.getExecutionHistory(arn);
    } catch (err) {
      sfnError = (err as Error).message ?? "GetExecutionHistory failed";
    }
    if (history) {
      files.push({ path: "sfn-history.json", content: JSON.stringify({ executionArn: arn, events: history }, null, 2) });
      statuses.push(status("sfn", history.length > 0 ? "present" : "empty", { count: history.length }));
    } else {
      statuses.push(
        status("sfn", sfnError ? "error" : "missing", {
          detail: sfnError ?? "execution not found (expired or never ran)"
        })
      );
    }
  }

  // --- 4. CloudWatch (best-effort, --cloudwatch only) ------------------
  if (flags.cloudwatch) {
    const endMs = toMs(meta.endedAt) ?? Date.now();
    const startMs = toMs(meta.startedAt) ?? endMs - flags.sinceMs;
    for (const [label, group] of Object.entries(targets.logGroups)) {
      let res: LogFetchResult;
      try {
        res = await sources.filterLogs(group, runId, startMs, endMs);
      } catch (err) {
        res = { kind: "error", message: (err as Error).message ?? "FilterLogEvents failed" };
      }
      const src = `cloudwatch:${label}`;
      if (res.kind === "events") {
        files.push({ path: `cloudwatch/${label}.ndjson`, content: ndjsonOf(res.events) });
        statuses.push(status(src, res.events.length > 0 ? "present" : "empty", { count: res.events.length }));
      } else if (res.kind === "empty") {
        statuses.push(status(src, "empty", { detail: `no events in window for ${group}` }));
      } else if (res.kind === "expired") {
        statuses.push(status(src, "expired", { detail: `log group ${group} not found / retention expired` }));
      } else {
        statuses.push(status(src, "error", { detail: res.message }));
      }
    }
  } else {
    statuses.push(status("cloudwatch", "skipped", { detail: "pass --cloudwatch to fetch (fresh runs only)" }));
  }

  // --- 5. Recurse into child/subagent runs -----------------------------
  const childRunIds = extractChildRunIds(events, runId).filter((id) => !visited.has(id));
  const children: RunManifest[] = [];
  for (const childId of childRunIds) {
    if (visited.has(childId)) continue;
    const childBundle = await assembleOne(childId, targets, sources, flags, visited);
    for (const f of childBundle.files) files.push({ path: `children/${childId}/${f.path}`, content: f.content });
    children.push(childBundle.manifest);
  }

  const manifest: RunManifest = {
    runId,
    generatedAt: new Date().toISOString(),
    plane: targets.plane,
    region: targets.region,
    outputsBucket: targets.outputsBucket,
    eventsArchiveBucket: targets.eventsArchiveBucket,
    runStatus: meta.status,
    startedAt: meta.startedAt,
    endedAt: meta.endedAt,
    journalSource,
    journalEventCount: events.length,
    childRunIds,
    sources: statuses,
    children
  };
  files.push({ path: "manifest.json", content: JSON.stringify(manifest, null, 2) });
  files.push({ path: "index.md", content: renderIndexMd(manifest) });
  return { files, manifest };
}

function ndjsonOf(rows: readonly unknown[]): string {
  return rows.length === 0 ? "" : rows.map((r) => JSON.stringify(r)).join("\n") + "\n";
}

function toMs(iso: string | null): number | null {
  if (!iso) return null;
  const ms = Date.parse(iso);
  return Number.isFinite(ms) ? ms : null;
}

// ---------------------------------------------------------------------------
// Write the assembled bundle to disk.
// ---------------------------------------------------------------------------

export async function writeDebugBundle(io: CliIO, baseDir: string, bundle: RunBundle): Promise<void> {
  const encoder = new TextEncoder();
  for (const file of bundle.files) {
    const abs = resolvePath(baseDir, file.path);
    if (io.mkdirp) await io.mkdirp(dirname(abs));
    await io.writeFile(abs, encoder.encode(file.content));
  }
}

// ---------------------------------------------------------------------------
// AWS SDK v3 wiring (lazy, externalized from the default CLI bundle).
// ---------------------------------------------------------------------------

interface AwsClient {
  send(command: unknown): Promise<Record<string, unknown>>;
}
type AwsClientCtor = new (cfg: { region: string }) => AwsClient;
type AwsCommandCtor = new (input: Record<string, unknown>) => unknown;

async function loadModule(pkg: string): Promise<Record<string, unknown>> {
  try {
    // Non-literal specifier: kept out of the esbuild bundle so the default
    // CLI install never carries the AWS SDK; resolved from node_modules here.
    return (await import(pkg)) as Record<string, unknown>;
  } catch (err) {
    throw new Error(
      `aex debug needs the AWS SDK v3 package "${pkg}". Install it (e.g. \`npm i ${pkg}\`) and retry. (${(err as Error).message})`
    );
  }
}

function isNotFound(err: unknown): boolean {
  const name = (err as { name?: string } | undefined)?.name ?? "";
  return /NotFound|NoSuchKey|NoSuchBucket|ResourceNotFound|ExecutionDoesNotExist/i.test(name);
}

function partitionForRegion(region: string): string {
  if (region.startsWith("cn-")) return "aws-cn";
  if (region.startsWith("us-gov-")) return "aws-us-gov";
  return "aws";
}

/** Recursively unmarshall a DynamoDB AttributeValue into a plain JS value. */
function unmarshallAttr(av: Record<string, unknown>): unknown {
  if ("S" in av) return av.S;
  if ("N" in av) return Number(av.N);
  if ("BOOL" in av) return av.BOOL;
  if ("NULL" in av) return null;
  if ("M" in av) {
    const m = av.M as Record<string, Record<string, unknown>>;
    const out: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(m)) out[k] = unmarshallAttr(v);
    return out;
  }
  if ("L" in av) return (av.L as Record<string, unknown>[]).map(unmarshallAttr);
  if ("SS" in av) return av.SS;
  if ("NS" in av) return (av.NS as string[]).map(Number);
  if ("B" in av) return av.B;
  return av;
}

function unmarshallItem(item: Record<string, Record<string, unknown>>): JournalEvent {
  const out: JournalEvent = {};
  for (const [k, v] of Object.entries(item)) out[k] = unmarshallAttr(v);
  return out;
}

export function makeAwsSources(region: string): AwsDebugClient {
  const clients = new Map<string, AwsClient>();
  const modules = new Map<string, Record<string, unknown>>();

  async function mod(pkg: string): Promise<Record<string, unknown>> {
    let m = modules.get(pkg);
    if (!m) {
      m = await loadModule(pkg);
      modules.set(pkg, m);
    }
    return m;
  }
  async function client(pkg: string, ctorName: string): Promise<AwsClient> {
    let c = clients.get(pkg);
    if (!c) {
      const m = await mod(pkg);
      const Ctor = m[ctorName] as AwsClientCtor;
      c = new Ctor({ region });
      clients.set(pkg, c);
    }
    return c;
  }

  async function listBuckets(): Promise<string[]> {
    const m = await mod("@aws-sdk/client-s3");
    const c = await client("@aws-sdk/client-s3", "S3Client");
    const Cmd = m.ListBucketsCommand as AwsCommandCtor;
    const out = await c.send(new Cmd({}));
    const buckets = (out.Buckets as { Name?: string }[] | undefined) ?? [];
    return buckets.map((b) => b.Name ?? "").filter((n) => n !== "");
  }

  return {
    async resolveTargets(plane, regionArg, accountHint): Promise<DebugTargets> {
      const prefix = `aex-${plane}-${regionArg}`;
      let account = accountHint;
      let outputsBucket = account ? `${prefix}-outputs-${account}` : "";
      let eventsArchiveBucket = account ? `${prefix}-events-archive-${account}` : "";
      if (!account) {
        const names = await listBuckets();
        const out = names.find((n) => n.startsWith(`${prefix}-outputs-`));
        const arch = names.find((n) => n.startsWith(`${prefix}-events-archive-`));
        if (out) {
          outputsBucket = out;
          account = out.slice(`${prefix}-outputs-`.length) || null;
        }
        if (arch) eventsArchiveBucket = arch;
      }
      if (!outputsBucket) outputsBucket = `${prefix}-outputs-${account ?? "unknown"}`;
      if (!eventsArchiveBucket) eventsArchiveBucket = `${prefix}-events-archive-${account ?? "unknown"}`;
      return {
        plane,
        region: regionArg,
        partition: partitionForRegion(regionArg),
        account: account ?? null,
        outputsBucket,
        eventsArchiveBucket,
        eventsTable: `${prefix}-events`,
        stateMachineName: `${prefix}-sfn`,
        logGroups: {
          api: `/aws/lambda/${prefix}-api`,
          sfn: `/aws/states/${prefix}-run`,
          brain: `/aws/ecs/${prefix}-brain`,
          egress: `/aws/ecs/${prefix}-egress`
        }
      };
    },

    async listObjects(bucket, keyPrefix): Promise<S3ListItem[]> {
      const m = await mod("@aws-sdk/client-s3");
      const c = await client("@aws-sdk/client-s3", "S3Client");
      const Cmd = m.ListObjectsV2Command as AwsCommandCtor;
      const items: S3ListItem[] = [];
      let token: string | undefined;
      do {
        const out = await c.send(
          new Cmd({ Bucket: bucket, Prefix: keyPrefix, ...(token ? { ContinuationToken: token } : {}) })
        );
        const contents = (out.Contents as { Key?: string; Size?: number }[] | undefined) ?? [];
        for (const o of contents) if (o.Key) items.push({ key: o.Key, sizeBytes: o.Size ?? 0 });
        token = out.IsTruncated ? (out.NextContinuationToken as string | undefined) : undefined;
      } while (token);
      return items;
    },

    async getObjectText(bucket, key): Promise<string | null> {
      const m = await mod("@aws-sdk/client-s3");
      const c = await client("@aws-sdk/client-s3", "S3Client");
      const Cmd = m.GetObjectCommand as AwsCommandCtor;
      try {
        const out = await c.send(new Cmd({ Bucket: bucket, Key: key }));
        const body = out.Body as { transformToString?: () => Promise<string> } | undefined;
        if (body?.transformToString) return await body.transformToString();
        return null;
      } catch (err) {
        if (isNotFound(err)) return null;
        throw err;
      }
    },

    async queryEvents(table, runId): Promise<JournalEvent[] | null> {
      try {
        const m = await mod("@aws-sdk/client-dynamodb");
        const c = await client("@aws-sdk/client-dynamodb", "DynamoDBClient");
        const Cmd = m.QueryCommand as AwsCommandCtor;
        const items: JournalEvent[] = [];
        let startKey: Record<string, unknown> | undefined;
        do {
          const out = await c.send(
            new Cmd({
              TableName: table,
              KeyConditionExpression: "runId = :r",
              ExpressionAttributeValues: { ":r": { S: runId } },
              ScanIndexForward: true,
              ...(startKey ? { ExclusiveStartKey: startKey } : {})
            })
          );
          const rows = (out.Items as Record<string, Record<string, unknown>>[] | undefined) ?? [];
          for (const r of rows) items.push(unmarshallItem(r));
          startKey = out.LastEvaluatedKey as Record<string, unknown> | undefined;
        } while (startKey);
        return items;
      } catch (err) {
        if (isNotFound(err)) return null;
        // Resilient: treat any DDB read failure as "no usable DDB data" so the
        // assembler falls back to the events-archive cold log.
        return null;
      }
    },

    async getExecutionHistory(executionArn): Promise<unknown[] | null> {
      try {
        const m = await mod("@aws-sdk/client-sfn");
        const c = await client("@aws-sdk/client-sfn", "SFNClient");
        const Cmd = m.GetExecutionHistoryCommand as AwsCommandCtor;
        const events: unknown[] = [];
        let token: string | undefined;
        do {
          const out = await c.send(
            new Cmd({ executionArn, maxResults: 1000, includeExecutionData: true, ...(token ? { nextToken: token } : {}) })
          );
          const page = (out.events as unknown[] | undefined) ?? [];
          events.push(...page);
          token = out.nextToken as string | undefined;
        } while (token);
        return events;
      } catch (err) {
        if (isNotFound(err)) return null;
        throw err;
      }
    },

    async filterLogs(logGroup, runId, startMs, endMs): Promise<LogFetchResult> {
      try {
        const m = await mod("@aws-sdk/client-cloudwatch-logs");
        const c = await client("@aws-sdk/client-cloudwatch-logs", "CloudWatchLogsClient");
        const Cmd = m.FilterLogEventsCommand as AwsCommandCtor;
        const events: unknown[] = [];
        let token: string | undefined;
        let pages = 0;
        do {
          const out = await c.send(
            new Cmd({
              logGroupName: logGroup,
              startTime: startMs,
              endTime: endMs,
              filterPattern: `"${runId}"`,
              ...(token ? { nextToken: token } : {})
            })
          );
          const page = (out.events as unknown[] | undefined) ?? [];
          events.push(...page);
          token = out.nextToken as string | undefined;
        } while (token && ++pages < 10);
        return events.length > 0 ? { kind: "events", events } : { kind: "empty" };
      } catch (err) {
        if (isNotFound(err)) return { kind: "expired" };
        return { kind: "error", message: (err as Error).message ?? "FilterLogEvents failed" };
      }
    }
  };
}

// ---------------------------------------------------------------------------
// Command entrypoint.
// ---------------------------------------------------------------------------

const USAGE =
  "usage: aex debug <run-id> [--plane dev|prd] [--region eu-west-2] [--out dir] " +
  "[--account <id>] [--cloudwatch] [--since <dur>] [--with-outputs]\n" +
  "  operator command — uses the standard AWS SDK credential chain (env/profile), NOT --api-token\n";

export async function runDebugCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  if (await refuseInsideManagedRun(io, "debug")) return USAGE_ERR;

  const planeF = takeOptionFlag(argv, "--plane");
  const regionF = takeOptionFlag(planeF.remaining, "--region");
  const outF = takeOptionFlag(regionF.remaining, "--out");
  const accountF = takeOptionFlag(outF.remaining, "--account");
  const sinceF = takeOptionFlag(accountF.remaining, "--since");
  const cwF = takeBooleanFlag(sinceF.remaining, "--cloudwatch");
  const woF = takeBooleanFlag(cwF.remaining, "--with-outputs");

  const rest = woF.remaining;
  const unknownFlags = rest.filter((a) => a.startsWith("--"));
  if (unknownFlags.length > 0) {
    io.stderr(`unknown flag(s): ${unknownFlags.join(", ")}\n${USAGE}`);
    return USAGE_ERR;
  }
  const positional = rest.filter((a) => !a.startsWith("--"));
  if (positional.length !== 1) {
    io.stderr(USAGE);
    return USAGE_ERR;
  }
  const runId = positional[0]!;

  const plane = planeF.value ?? "dev";
  if (plane !== "dev" && plane !== "prd") {
    io.stderr(`--plane must be one of: dev, prd (got "${plane}")\n`);
    return USAGE_ERR;
  }
  const region = regionF.value ?? "eu-west-2";

  let sinceMs = 24 * 60 * 60 * 1000;
  if (sinceF.value !== undefined) {
    const d = parseDuration(sinceF.value);
    if (d.error || d.ms === null) {
      io.stderr(`--since: ${d.error ?? "invalid duration"}\n`);
      return USAGE_ERR;
    }
    sinceMs = d.ms;
  }

  const outDir = resolvePath(io.cwd(), outF.value ?? `aex-debug-${runId}`);
  const client = makeAwsSources(region);

  let targets: DebugTargets;
  try {
    targets = await client.resolveTargets(plane, region, accountF.value ?? null);
  } catch (err) {
    io.stderr(
      JSON.stringify({
        error: "debug_resolve_failed",
        message: (err as Error).message ?? "failed to resolve plane targets",
        hint: "check AWS credentials (env/profile) or pass --account <id>"
      }) + "\n"
    );
    return RUNTIME_ERR;
  }

  let bundle: RunBundle;
  try {
    bundle = await assembleBundle({
      runId,
      targets,
      sources: client,
      cloudwatch: cwF.present,
      withOutputs: woF.present,
      sinceMs
    });
  } catch (err) {
    io.stderr(JSON.stringify({ error: "debug_assemble_failed", message: (err as Error).message ?? "assembly failed" }) + "\n");
    return RUNTIME_ERR;
  }

  try {
    await writeDebugBundle(io, outDir, bundle);
  } catch (err) {
    io.stderr(
      JSON.stringify({ error: "debug_write_failed", message: (err as Error).message ?? "write failed", outDir }) + "\n"
    );
    return RUNTIME_ERR;
  }

  io.stdout(
    JSON.stringify(
      {
        runId,
        plane,
        region,
        outDir,
        fileCount: bundle.files.length,
        runStatus: bundle.manifest.runStatus,
        journalSource: bundle.manifest.journalSource,
        journalEventCount: bundle.manifest.journalEventCount,
        childRunIds: bundle.manifest.childRunIds,
        sources: bundle.manifest.sources
      },
      null,
      2
    ) + "\n"
  );
  return SUCCESS;
}
