/**
 * C4 — the wire-conformance harness.
 *
 * Validates the bytes a real server actually returned against the response
 * schemas. This is the only gate in the contract pipeline that observes reality
 * rather than comparing two of our own artefacts to each other, and it is the
 * one the plan flags as easiest to skip and most costly to skip.
 *
 * It attaches to `HttpClient` through {@link observeWireResponses}, so every
 * response the live and user-test suites already receive is checked — no new
 * suite, no new requests, no server changes.
 *
 * **Coverage is reported, not assumed.** A green run over 12 of 120 routes is
 * not a verified surface, and calling it one is the specific failure this
 * harness exists to prevent. {@link WireConformanceReport} names the routes that
 * were checked, the routes that have a schema but were never exercised, the
 * responses that arrived with no schema to check them against, and the responses
 * that were deliberately skipped because they came from another plane.
 *
 * ## What it checks, and what it used to miss
 *
 * - **2xx bodies** against the operation's response schema.
 * - **non-2xx bodies** against {@link ApiErrorEnvelopeSchema}. Every generated
 *   operation declares that envelope, and until `HttpClient` learned to report
 *   before it throws, nothing had ever checked it.
 *
 * ## The plane collision, and how it is closed
 *
 * A PATH does not identify a route. The control plane serves different bodies at
 * two of the data plane's paths (`GET /api/whoami`,
 * `DELETE /api/workspaces/{id}`), so a process driving both planes — the CLI
 * does, `aex login` is control-plane — would validate a control-plane body
 * against a data-plane schema and report a violation that is not one.
 * {@link WireResponse} therefore carries the ORIGIN it came from, and
 * {@link WireConformanceOptions.origin} says which plane the bindings describe.
 * Responses from anywhere else are counted and named under
 * {@link WireConformanceReport.offPlane} rather than silently dropped.
 *
 * Installing WITHOUT an origin is still allowed, because a data-plane-only suite
 * has nothing to collide with — but the report says so in as many words, so a
 * reader can tell a filtered run from an unfiltered one.
 */
import type { StandardSchemaV1 } from "@standard-schema/spec";
import { ApiErrorEnvelopeSchema } from "../schemas/response-common.js";
import { observeWireResponses, type WireResponse } from "../wire-observer.js";

/** A response schema bound to the route whose responses it describes. */
export interface ResponseSchemaBinding {
  /** Uppercase HTTP method. */
  readonly method: string;
  /**
   * Path pattern with `{param}` placeholders, matched segment-wise:
   * `/api/sessions/{sessionId}/messages`.
   */
  readonly path: string;
  /** Operation id, used in the report. */
  readonly name: string;
  readonly schema: StandardSchemaV1;
}

export interface WireConformanceOptions {
  /**
   * The plane the bindings describe, as an origin or any URL on it —
   * `https://dev-api.aex.dev`, or the `AEX_API_URL` the suite already reads.
   *
   * Responses from any other origin are recorded in
   * {@link WireConformanceReport.offPlane} and NOT validated. Omit only when the
   * process provably drives one plane; the report states which case it was.
   */
  readonly origin?: string | undefined;
  /**
   * Schema for non-2xx bodies. Defaults to {@link ApiErrorEnvelopeSchema} — the
   * shape the generated document declares as every operation's `default`
   * response. Pass `null` to stop checking error bodies, which throws away the
   * only assertion that covers 68 declared-but-unchecked responses; there is no
   * good reason to.
   */
  readonly errorEnvelope?: StandardSchemaV1 | null;
}

export interface WireConformanceViolation {
  /**
   * Which contract was broken: the operation's own 2xx schema, or the error
   * envelope every operation declares.
   */
  readonly kind: "response" | "error-envelope";
  /** Operation id, or `METHOD /path` when no binding matched. */
  readonly name: string;
  readonly method: string;
  readonly origin: string;
  readonly path: string;
  readonly status: number;
  readonly issues: readonly string[];
  /**
   * The body that failed, verbatim.
   *
   * Carried because a complaint without the bytes that caused it cannot be
   * triaged — the reader has to be able to tell a server defect from a schema
   * defect, and only the actual body settles that.
   */
  readonly body: unknown;
}

export interface WireConformanceReport {
  /**
   * The origin responses were required to come from, or `undefined` when the
   * harness was installed without a plane filter.
   */
  readonly originFilter: string | undefined;
  /** Operation ids whose 2xx responses were seen and validated. */
  readonly validated: readonly string[];
  /** Operation ids with a response schema that no request exercised. */
  readonly unexercised: readonly string[];
  /** `METHOD /path` for responses that arrived with no schema bound. */
  readonly unschemad: readonly string[];
  /** `METHOD /path -> status` for non-2xx bodies checked against the envelope. */
  readonly errorsValidated: readonly string[];
  /** `origin METHOD /path` for responses from a plane these bindings do not describe. */
  readonly offPlane: readonly string[];
  readonly violations: readonly WireConformanceViolation[];
  /** Responses observed in total, including ones with no schema and off-plane ones. */
  readonly observed: number;
}

/**
 * Match a concrete path against a `{param}` pattern.
 *
 * Segment-wise rather than by regex so a placeholder cannot accidentally span a
 * `/` and make two different routes look like one.
 */
export function pathMatches(pattern: string, actual: string): boolean {
  const patternSegments = pattern.split("/");
  const actualSegments = actual.split("/");
  if (patternSegments.length !== actualSegments.length) {
    return false;
  }
  return patternSegments.every(
    (segment, index) =>
      (segment.startsWith("{") && segment.endsWith("}")) || segment === actualSegments[index]
  );
}

/**
 * Reduce whatever the caller had to hand — a base URL, a URL with a path, a bare
 * origin — to the origin `WireResponse` reports.
 *
 * Throws rather than falling back to "match everything": an unparseable
 * `AEX_API_URL` that silently disabled the filter would reintroduce exactly the
 * false violations the filter exists to prevent, and would do it quietly.
 */
export function wireOrigin(baseUrl: string): string {
  try {
    return new URL(baseUrl).origin;
  } catch (cause) {
    throw new Error(
      `wire conformance: could not read an origin from ${JSON.stringify(baseUrl)} — ` +
        `expected an absolute URL like "https://api.aex.dev"`,
      { cause }
    );
  }
}

function issuesOf(result: StandardSchemaV1.Result<unknown>): readonly string[] {
  return (result.issues ?? []).map((issue) => {
    const path = (issue.path ?? [])
      .map((segment) => (typeof segment === "object" ? String(segment.key) : String(segment)))
      .join(".");
    return path ? `${path}: ${issue.message}` : issue.message;
  });
}

/**
 * Which contract a status is judged against.
 *
 * The generated document is coarser than this — it declares `2XX` and a
 * `default` that catches everything else — but judging a redirect against the
 * error envelope would manufacture a violation out of an empty body, which is
 * noise rather than a finding. A 3xx is recorded, named and not judged.
 *
 * In practice `fetch` follows redirects, so `HttpClient` sees the final
 * response; this exists so that a caller who stops following them gets a
 * comprehensible report rather than a wall of false failures.
 */
function contractFor(status: number): "response" | "error-envelope" | "redirect" {
  if (status < 300) return "response";
  if (status < 400) return "redirect";
  return "error-envelope";
}

/**
 * Start validating responses.
 *
 * Returns a handle carrying the report and a `stop()`. Validation is
 * synchronous: a schema whose `~standard.validate` returns a promise is treated
 * as unvalidatable and recorded as such rather than silently skipped, because
 * `HttpClient` cannot await an observer without changing request timing.
 */
export function installWireConformance(
  bindings: readonly ResponseSchemaBinding[],
  options: WireConformanceOptions = {}
): {
  readonly report: () => WireConformanceReport;
  readonly stop: () => void;
} {
  const originFilter = options.origin === undefined ? undefined : wireOrigin(options.origin);
  const errorEnvelope =
    options.errorEnvelope === undefined ? ApiErrorEnvelopeSchema : options.errorEnvelope;
  const validated = new Set<string>();
  const unschemad = new Set<string>();
  const errorsValidated = new Set<string>();
  const offPlane = new Set<string>();
  const violations: WireConformanceViolation[] = [];
  let observed = 0;

  /** Validate `body`, recording a violation on failure. Returns false if async. */
  const check = (
    schema: StandardSchemaV1,
    kind: WireConformanceViolation["kind"],
    name: string,
    response: WireResponse
  ): boolean => {
    const result = schema["~standard"].validate(response.body);
    if (result instanceof Promise) {
      return false;
    }
    if (result.issues) {
      violations.push({
        kind,
        name,
        method: response.method.toUpperCase(),
        origin: response.origin,
        path: response.path,
        status: response.status,
        issues: issuesOf(result),
        body: response.body
      });
    }
    return true;
  };

  const stop = observeWireResponses((response: WireResponse) => {
    observed += 1;
    const method = response.method.toUpperCase();
    if (originFilter !== undefined && response.origin !== originFilter) {
      offPlane.add(`${response.origin} ${method} ${response.path}`);
      return;
    }

    const contract = contractFor(response.status);
    if (contract === "redirect") {
      unschemad.add(`${method} ${response.path} -> ${response.status} (redirect — no declared body)`);
      return;
    }
    if (contract === "error-envelope") {
      // An error body is the SAME envelope whatever route produced it, so it is
      // checked without needing a binding — which is the point: the routes with
      // no 2xx schema still have their failures covered.
      if (errorEnvelope === null) {
        return;
      }
      if (check(errorEnvelope, "error-envelope", `${method} ${response.path}`, response)) {
        errorsValidated.add(`${method} ${response.path} -> ${response.status}`);
      } else {
        unschemad.add(`${method} ${response.path} -> ${response.status} (async schema — not validated)`);
      }
      return;
    }

    const binding = bindings.find(
      (candidate) => candidate.method === method && pathMatches(candidate.path, response.path)
    );
    if (!binding) {
      unschemad.add(`${method} ${response.path}`);
      return;
    }
    if (!check(binding.schema, "response", binding.name, response)) {
      unschemad.add(`${binding.name} (async schema — not validated)`);
      return;
    }
    validated.add(binding.name);
  });

  return {
    stop,
    report: () => ({
      originFilter,
      validated: [...validated].sort(),
      unexercised: bindings
        .map((binding) => binding.name)
        .filter((name) => !validated.has(name))
        .sort(),
      unschemad: [...unschemad].sort(),
      errorsValidated: [...errorsValidated].sort(),
      offPlane: [...offPlane].sort(),
      violations,
      observed
    })
  };
}

/**
 * Fold reports from several processes into one.
 *
 * The suites that matter drive the SDK out of process — one bun child per
 * scenario, one test process per file — so no single process sees the whole
 * surface, and a per-process report would understate coverage by construction.
 *
 * `unexercised` is INTERSECTED, not concatenated: a name is unexercised overall
 * only when every fragment failed to exercise it. Concatenating would report a
 * route as unexercised because some other process did not happen to call it,
 * which is the mirror image of the false-green this harness exists to prevent.
 * That identity only holds when every fragment was produced from the same
 * binding table; {@link installWireConformance} is the only producer.
 */
export function mergeWireConformanceReports(
  reports: readonly WireConformanceReport[]
): WireConformanceReport {
  const validated = new Set<string>();
  const unschemad = new Set<string>();
  const errorsValidated = new Set<string>();
  const offPlane = new Set<string>();
  const violations: WireConformanceViolation[] = [];
  const originFilters = new Set<string>();
  let unexercised: Set<string> | undefined;
  let observed = 0;
  let anyUnfiltered = false;

  for (const report of reports) {
    observed += report.observed;
    for (const name of report.validated) validated.add(name);
    for (const route of report.unschemad) unschemad.add(route);
    for (const route of report.errorsValidated) errorsValidated.add(route);
    for (const route of report.offPlane) offPlane.add(route);
    violations.push(...report.violations);
    if (report.originFilter === undefined) anyUnfiltered = true;
    else originFilters.add(report.originFilter);
    unexercised =
      unexercised === undefined
        ? new Set(report.unexercised)
        : new Set(report.unexercised.filter((name) => unexercised!.has(name)));
  }

  return {
    // One filter across every fragment is the only case that can be stated
    // simply. A mixed run is reported as unfiltered so the formatter's warning
    // fires rather than a filter being claimed that not every fragment applied.
    originFilter: !anyUnfiltered && originFilters.size === 1 ? [...originFilters][0] : undefined,
    validated: [...validated].sort(),
    unexercised: [...(unexercised ?? new Set<string>())].filter((name) => !validated.has(name)).sort(),
    unschemad: [...unschemad].sort(),
    errorsValidated: [...errorsValidated].sort(),
    offPlane: [...offPlane].sort(),
    violations,
    observed
  };
}

/** Render a body for a violation without letting one huge response bury the report. */
function renderBody(body: unknown, limit = 2000): string {
  let text: string;
  try {
    text = JSON.stringify(body) ?? String(body);
  } catch {
    text = String(body);
  }
  return text.length > limit ? `${text.slice(0, limit)}… (${text.length} bytes total)` : text;
}

/**
 * Render the report for a suite's output.
 *
 * Always prints the unexercised and unschemad lists, including when there are no
 * violations — a run that reports only "0 violations" invites the reader to
 * conclude the surface is verified. A violation always prints the BODY that
 * caused it as well as the schema's complaint, because triage needs both.
 */
export function formatWireConformanceReport(report: WireConformanceReport): string {
  const lines = [
    `wire conformance: ${report.observed} response(s) observed, ` +
      `${report.validated.length} operation(s) validated, ` +
      `${report.errorsValidated.length} error response(s) checked, ` +
      `${report.violations.length} violation(s)`
  ];
  lines.push(
    report.originFilter === undefined
      ? `  ORIGIN FILTER: none — every observed response was matched by PATH alone. ` +
          `Sound only for a process that drives ONE plane; the control plane serves ` +
          `different bodies at GET /api/whoami and DELETE /api/workspaces/{id}.`
      : `  ORIGIN FILTER: ${report.originFilter}`
  );
  if (report.offPlane.length > 0) {
    lines.push(`  OFF-PLANE (other origin, not checked against these bindings):`);
    for (const route of report.offPlane) {
      lines.push(`    - ${route}`);
    }
  }
  if (report.validated.length > 0) {
    lines.push(`  VALIDATED (${report.validated.length}): ${report.validated.join(", ")}`);
  }
  if (report.errorsValidated.length > 0) {
    lines.push(`  ERROR ENVELOPES CHECKED (${report.errorsValidated.length}):`);
    for (const route of report.errorsValidated) {
      lines.push(`    - ${route}`);
    }
  }
  if (report.unexercised.length > 0) {
    lines.push(
      `  NOT EXERCISED (schema exists, no response seen) (${report.unexercised.length}): ` +
        `${report.unexercised.join(", ")}`
    );
  }
  if (report.unschemad.length > 0) {
    lines.push(`  NO SCHEMA (response seen, nothing to check it against):`);
    for (const route of report.unschemad) {
      lines.push(`    - ${route}`);
    }
  }
  for (const violation of report.violations) {
    lines.push(
      `  VIOLATION [${violation.kind}] ${violation.name} ` +
        `(${violation.method} ${violation.origin}${violation.path} -> ${violation.status}):`
    );
    for (const issue of violation.issues) {
      lines.push(`    - ${issue}`);
    }
    lines.push(`    body: ${renderBody(violation.body)}`);
  }
  return lines.join("\n");
}
