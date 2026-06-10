import { parseDurationToMs } from "./runtime-sizes.js";

/** Default post-agent-run hook timeout (5 minutes). */
export const DEFAULT_POST_HOOK_TIMEOUT_MS = 5 * 60 * 1000;

/** Default number of repair turns after a failing hook. */
export const DEFAULT_POST_HOOK_MAX_TURNS = 10;

/**
 * Customer-facing post-agent-run verifier shape. The SDK and run-config JSON
 * send this form on the wire; the server parser normalizes it to
 * {@link PlatformPostHook}.
 */
export interface PlatformPostHookInput {
  readonly command: string;
  readonly timeout?: string;
  readonly maxTurns?: number;
  readonly maxChars?: number | null;
}

/** Parsed post-agent-run verifier carried through dispatch to the runner. */
export interface PlatformPostHook {
  readonly command: string;
  readonly timeoutMs: number;
  readonly maxTurns: number;
  readonly maxChars: number | null;
}

/**
 * Parse the public `postHook` option. An omitted hook, null hook, or hook with
 * an empty/whitespace-only command is treated as omitted so callers can pre-fill
 * configs without enabling the verifier accidentally.
 */
export function parsePostHook(input: unknown, path = "postHook"): PlatformPostHook | undefined {
  if (input === undefined || input === null) {
    return undefined;
  }
  const value = requirePostHookRecord(input, path);
  const allowed = new Set(["command", "timeout", "maxTurns", "maxChars"]);
  for (const key of Object.keys(value)) {
    if (!allowed.has(key)) {
      throw new Error(`${path}.${key} is not an allowed field; permitted: command, timeout, maxTurns, maxChars`);
    }
  }
  if (typeof value.command !== "string") {
    throw new Error(`${path}.command must be a string`);
  }
  if (value.command.trim().length === 0) {
    return undefined;
  }
  const timeoutMs = parsePostHookTimeout(value.timeout, `${path}.timeout`);
  const maxTurns = parseNonNegativeInteger(
    value.maxTurns,
    `${path}.maxTurns`,
    DEFAULT_POST_HOOK_MAX_TURNS
  );
  const maxChars =
    value.maxChars === null
      ? null
      : parseNonNegativeInteger(value.maxChars, `${path}.maxChars`, null);

  return {
    command: value.command,
    timeoutMs,
    maxTurns,
    maxChars
  };
}

function parsePostHookTimeout(input: unknown, path: string): number {
  if (input === undefined) {
    return DEFAULT_POST_HOOK_TIMEOUT_MS;
  }
  if (typeof input !== "string") {
    throw new Error(`${path} must be a duration string (e.g. "5m", "30s"); got ${JSON.stringify(input)}`);
  }
  const ms = parseDurationToMs(input);
  if (ms <= 0) {
    throw new Error(`${path} must be greater than 0ms; got ${ms}ms`);
  }
  return ms;
}

function parseNonNegativeInteger(
  input: unknown,
  path: string,
  defaultValue: number
): number;
function parseNonNegativeInteger(
  input: unknown,
  path: string,
  defaultValue: null
): number | null;
function parseNonNegativeInteger(
  input: unknown,
  path: string,
  defaultValue: number | null
): number | null {
  if (input === undefined) {
    return defaultValue;
  }
  if (typeof input !== "number" || !Number.isSafeInteger(input) || input < 0) {
    throw new Error(`${path} must be a non-negative integer`);
  }
  return input;
}

function requirePostHookRecord(input: unknown, path: string): Record<string, unknown> {
  if (input === null || typeof input !== "object" || Array.isArray(input)) {
    throw new Error(`${path} must be an object`);
  }
  return input as Record<string, unknown>;
}
