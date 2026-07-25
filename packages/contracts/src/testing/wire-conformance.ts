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
 * were checked, the routes that have a schema but were never exercised, and the
 * responses that arrived with no schema to check them against.
 */
import type { StandardSchemaV1 } from "@standard-schema/spec";
import {
  observeWireResponses,
  type WireResponse
} from "../wire-observer.js";

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

export interface WireConformanceViolation {
  readonly name: string;
  readonly method: string;
  readonly path: string;
  readonly status: number;
  readonly issues: readonly string[];
}

export interface WireConformanceReport {
  /** Operation ids whose responses were seen and validated. */
  readonly validated: readonly string[];
  /** Operation ids with a response schema that no request exercised. */
  readonly unexercised: readonly string[];
  /** `METHOD /path` for responses that arrived with no schema bound. */
  readonly unschemad: readonly string[];
  readonly violations: readonly WireConformanceViolation[];
  /** Responses observed in total, including ones with no schema. */
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

function issuesOf(result: StandardSchemaV1.Result<unknown>): readonly string[] {
  return (result.issues ?? []).map((issue) => {
    const path = (issue.path ?? [])
      .map((segment) => (typeof segment === "object" ? String(segment.key) : String(segment)))
      .join(".");
    return path ? `${path}: ${issue.message}` : issue.message;
  });
}

/**
 * Start validating responses.
 *
 * Returns a handle carrying the report and a `stop()`. Validation is
 * synchronous: a schema whose `~standard.validate` returns a promise is treated
 * as unvalidatable and recorded as such rather than silently skipped, because
 * `HttpClient` cannot await an observer without changing request timing.
 */
export function installWireConformance(bindings: readonly ResponseSchemaBinding[]): {
  readonly report: () => WireConformanceReport;
  readonly stop: () => void;
} {
  const validated = new Set<string>();
  const unschemad = new Set<string>();
  const violations: WireConformanceViolation[] = [];
  let observed = 0;

  const stop = observeWireResponses((response: WireResponse) => {
    observed += 1;
    const binding = bindings.find(
      (candidate) =>
        candidate.method === response.method.toUpperCase() &&
        pathMatches(candidate.path, response.path)
    );
    if (!binding) {
      unschemad.add(`${response.method.toUpperCase()} ${response.path}`);
      return;
    }
    const result = binding.schema["~standard"].validate(response.body);
    if (result instanceof Promise) {
      unschemad.add(`${binding.name} (async schema — not validated)`);
      return;
    }
    validated.add(binding.name);
    if (result.issues) {
      violations.push({
        name: binding.name,
        method: response.method.toUpperCase(),
        path: response.path,
        status: response.status,
        issues: issuesOf(result)
      });
    }
  });

  return {
    stop,
    report: () => ({
      validated: [...validated].sort(),
      unexercised: bindings
        .map((binding) => binding.name)
        .filter((name) => !validated.has(name))
        .sort(),
      unschemad: [...unschemad].sort(),
      violations,
      observed
    })
  };
}

/**
 * Render the report for a suite's output.
 *
 * Always prints the unexercised and unschemad lists, including when there are no
 * violations — a run that reports only "0 violations" invites the reader to
 * conclude the surface is verified.
 */
export function formatWireConformanceReport(report: WireConformanceReport): string {
  const lines = [
    `wire conformance: ${report.observed} response(s) observed, ` +
      `${report.validated.length} operation(s) validated, ${report.violations.length} violation(s)`
  ];
  if (report.unexercised.length > 0) {
    lines.push(
      `  NOT EXERCISED (schema exists, no response seen): ${report.unexercised.join(", ")}`
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
      `  VIOLATION ${violation.name} (${violation.method} ${violation.path} -> ${violation.status}):`
    );
    for (const issue of violation.issues) {
      lines.push(`    - ${issue}`);
    }
  }
  return lines.join("\n");
}
