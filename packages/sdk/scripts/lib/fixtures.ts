/**
 * Pure (I/O-free) core for the Anthropic record-replay fixtures.
 *
 * SCOPE HONESTY: these fixtures guard Anthropic Managed Agents *response-shape
 * EVOLUTION* — an inbound `ProviderEvent` field/type renamed, added, or removed
 * under us. They do NOT catch this cycle's seam bugs (the dotted event-type is
 * an OUTBOUND string the control plane validates; globalThis/undici are
 * client-runtime). See surface invariants §Record-replay.
 *
 * Three responsibilities, all pure so they unit-test offline:
 *   1. sanitize — strip every secret-SHAPED value via the shared value-agnostic
 *      redactor, and FLAG residual secrets (the security-critical gate).
 *   2. normalize — replace non-deterministic bits (ids, timestamps) with stable
 *      placeholders so replay + shape-diff are stable run-to-run.
 *   3. round-trip helpers for the raw/sanitized fixture envelopes.
 *
 * The shape-diff itself lives with the drift probe (scripts/validate/probes/
 * anthropic-event-shape.ts) — it is consumed by Tier 1, not the SDK build.
 */

import { containsSecretLikeValue, redactString } from "@antpath/contracts";

/** Current sanitized-fixture envelope version. Bumped only on a breaking
 * envelope change so a stale fixture is detected rather than silently replayed. */
export const FIXTURE_VERSION = 1 as const;

/** A single captured Managed Agents session event (whole poll-list object, not
 * an SSE delta — see the hosted Anthropic-native adapter contract). */
export type RecordedEvent = Record<string, unknown>;

/** The on-disk RAW recording (gitignored `*.raw.json`): captured verbatim,
 * may contain secrets. Never committed. */
export interface RawRecording {
  /** Free-form label describing the captured scenario (e.g. "simple-turn"). */
  readonly scenario: string;
  /** How the recording was produced (live capture vs derived seed). */
  readonly source: "live-capture" | "derived-seed";
  /** ISO timestamp of capture (normalized away in the sanitized output). */
  readonly capturedAt?: string;
  /** The whole-object session events, in poll order. */
  readonly events: readonly RecordedEvent[];
}

/** The committed, SANITIZED + NORMALIZED fixture replayed in unit tests and
 * diffed against live reality by the Tier-1 drift probe. */
export interface SanitizedFixture {
  readonly version: typeof FIXTURE_VERSION;
  readonly scenario: string;
  readonly source: RawRecording["source"];
  readonly events: readonly RecordedEvent[];
}

const TS_PLACEHOLDER = "1970-01-01T00:00:00.000Z";
/** ISO-8601 timestamp shape (the non-deterministic bits we normalize). */
const ISO_TIMESTAMP = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})$/;
/** Keys whose VALUES are opaque per-run ids — normalized to a stable token so
 * the diff/replay don't churn on a fresh capture (shape-not-values rule). */
const ID_KEYS = new Set(["id", "session_id", "tool_use_id", "request_id", "message_id", "parent_id"]);
/** Keys whose values are timestamps — normalized to a fixed instant. */
const TIMESTAMP_KEYS = new Set(["created_at", "processed_at", "updated_at", "started_at", "ended_at", "completed_at"]);

/**
 * Recursively sanitize a value: every string runs through the shared
 * value-AGNOSTIC redactor (`redactString`), so a secret is caught by SHAPE
 * whether or not we happen to hold its literal value. Object keys are not
 * redacted (they are field names, not secrets); their VALUES are.
 */
export function sanitizeValue<T>(value: T): T {
  if (typeof value === "string") return redactString(value) as T;
  if (Array.isArray(value)) return value.map((v) => sanitizeValue(v)) as T;
  if (value && typeof value === "object") {
    const out: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(value)) out[k] = sanitizeValue(v);
    return out as T;
  }
  return value;
}

/**
 * Normalize the non-deterministic bits so replay + shape-diff are stable across
 * captures. PRESERVES the value's TYPE (a normalized id stays a string, a
 * normalized number stays a number) — the drift diff asserts type, not value,
 * so over-normalizing would blind it. Recurses by key context.
 */
export function normalizeValue(value: unknown, keyContext?: string): unknown {
  if (typeof value === "string") {
    if (keyContext && TIMESTAMP_KEYS.has(keyContext)) return TS_PLACEHOLDER;
    if (ISO_TIMESTAMP.test(value)) return TS_PLACEHOLDER;
    if (keyContext && ID_KEYS.has(keyContext)) return `<${keyContext}>`;
    return value;
  }
  if (Array.isArray(value)) return value.map((v) => normalizeValue(v));
  if (value && typeof value === "object") {
    const out: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(value)) out[k] = normalizeValue(v, k);
    return out;
  }
  return value;
}

/** A residual secret found in a fixture, located by its JSON path. */
export interface ResidualSecret {
  readonly path: string;
  readonly sample: string;
}

/**
 * Walk a value and collect every string that STILL looks secret-shaped after
 * sanitizing. The sanitizer's fail-on-residual gate reads this: a non-empty
 * result means a secret survived and the run MUST exit non-zero. The sample is
 * truncated so the error itself never re-prints the full secret.
 */
export function findResidualSecrets(value: unknown, path = "$"): ResidualSecret[] {
  const found: ResidualSecret[] = [];
  if (typeof value === "string") {
    if (containsSecretLikeValue(value)) {
      found.push({ path, sample: value.length > 16 ? `${value.slice(0, 16)}…` : value });
    }
  } else if (Array.isArray(value)) {
    value.forEach((v, i) => found.push(...findResidualSecrets(v, `${path}[${i}]`)));
  } else if (value && typeof value === "object") {
    for (const [k, v] of Object.entries(value)) found.push(...findResidualSecrets(v, `${path}.${k}`));
  }
  return found;
}

/**
 * Produce a committed fixture from a raw recording: sanitize THEN normalize THEN
 * gate. Throws (so the CLI exits non-zero) if any secret-shaped value survives —
 * a sanitized fixture that still carries a secret must never be written.
 */
export function buildSanitizedFixture(raw: RawRecording): SanitizedFixture {
  const sanitizedEvents = raw.events.map((e) => normalizeValue(sanitizeValue(e)) as RecordedEvent);
  const residual = findResidualSecrets(sanitizedEvents);
  if (residual.length > 0) {
    throw new ResidualSecretError(residual);
  }
  return {
    version: FIXTURE_VERSION,
    scenario: raw.scenario,
    source: raw.source,
    events: sanitizedEvents
  };
}

/** Thrown when sanitized output still contains a secret-shaped value. The
 * message lists PATHS and truncated samples only — never the full secret. */
export class ResidualSecretError extends Error {
  readonly residual: readonly ResidualSecret[];
  constructor(residual: readonly ResidualSecret[]) {
    super(
      `sanitized fixture still contains ${residual.length} secret-shaped value(s): ` +
        residual.map((r) => `${r.path} (${r.sample})`).join(", ")
    );
    this.name = "ResidualSecretError";
    this.residual = residual;
  }
}

/** Parse + validate a sanitized fixture loaded from disk (replay/drift read
 * path). Throws on a wrong-version or structurally-bad fixture. */
export function parseSanitizedFixture(json: unknown): SanitizedFixture {
  if (!json || typeof json !== "object") throw new Error("fixture is not an object");
  const f = json as Record<string, unknown>;
  if (f.version !== FIXTURE_VERSION) {
    throw new Error(`fixture version ${String(f.version)} != expected ${FIXTURE_VERSION}`);
  }
  if (!Array.isArray(f.events)) throw new Error("fixture.events is not an array");
  return {
    version: FIXTURE_VERSION,
    scenario: typeof f.scenario === "string" ? f.scenario : "unknown",
    source: f.source === "live-capture" ? "live-capture" : "derived-seed",
    events: f.events as RecordedEvent[]
  };
}
