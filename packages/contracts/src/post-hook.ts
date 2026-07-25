import { parseDurationToMs } from "./runtime-sizes.js";
import { withContractParseError } from "./contract-parse-error.js";
import { postHookGateSchema, postHookSchema, type PostHookWire } from "./schemas/post-hook.js";
import { parseWire } from "./schemas/wire.js";

/** Default post-agent-run hook timeout (5 minutes). */
export const DEFAULT_POST_HOOK_TIMEOUT_MS = 5 * 60 * 1000;

/** Default number of repair turns after a failing hook. */
export const DEFAULT_POST_HOOK_MAX_TURNS = 10;

/**
 * Private post-agent-run verifier input. The public SDK does not
 * accepts this field; platform-internal paths may still normalize old wire
 * records to {@link PlatformPostHook}.
 *
 * The runtime statement of this shape is `postHookSchema()` in
 * `./schemas/post-hook.ts`; its keys are the accepted field list.
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
 * Parse the private `postHook` option. An omitted hook, null hook, or hook with
 * an empty/whitespace-only command is treated as omitted so callers can pre-fill
 * configs without enabling the verifier accidentally.
 */
export function parsePostHook(input: unknown, path = "postHook"): PlatformPostHook | undefined {
  return withContractParseError("parsePostHook", () => {
    if (input === undefined || input === null) {
      return undefined;
    }
    // Two passes, deliberately. The gate settles shape, the allow-list and the
    // command; only if the command is non-blank do the sibling rules run. A
    // single pass would reject `{ command: "", timeout: <junk> }`, which is
    // exactly the pre-filled config the blank-command rule exists to allow.
    if (parseWire(postHookGateSchema(path), input).command.trim().length === 0) {
      return undefined;
    }
    return normalizePostHook(parseWire(postHookSchema(path), input), path);
  });
}

/**
 * Apply the hook's defaults and turn its wire duration into milliseconds.
 *
 * Separate from the schema because both halves are transforms — per D4 a schema
 * that transformed could not be converted on the output side — and because the
 * duration parse belongs to {@link parseDurationToMs}, which brands its own
 * failures.
 *
 * The blank-command collapse is repeated here so this function is correct on its
 * own; the caller has already applied it via the gate schema, which is what
 * keeps a blank command from ever reaching the sibling field rules.
 */
function normalizePostHook(hook: PostHookWire, path: string): PlatformPostHook | undefined {
  if (hook.command.trim().length === 0) {
    return undefined;
  }
  return {
    command: hook.command,
    timeoutMs: postHookTimeoutMs(hook.timeout, `${path}.timeout`),
    maxTurns: hook.maxTurns ?? DEFAULT_POST_HOOK_MAX_TURNS,
    maxChars: hook.maxChars ?? null
  };
}

function postHookTimeoutMs(timeout: string | undefined, path: string): number {
  if (timeout === undefined) {
    return DEFAULT_POST_HOOK_TIMEOUT_MS;
  }
  const ms = parseDurationToMs(timeout);
  if (ms <= 0) {
    throw new Error(`${path} must be greater than 0ms; got ${ms}ms`);
  }
  return ms;
}
