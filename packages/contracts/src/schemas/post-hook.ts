/**
 * Schema for the private post-agent-run verifier hook.
 *
 * Shape gate only. Turning `timeout` into milliseconds, defaulting `maxTurns`,
 * and collapsing an empty command to "no hook" are all normalisation and live in
 * `../post-hook.ts` — a `.transform()` here would make the output half of the
 * generated spec ungenerable (D4/L1), and the duration parse must keep throwing
 * its own branded `parseDurationToMs` error rather than a re-worded schema one.
 */
import * as z from "zod/mini";
import { wireObject } from "./wire.js";

/**
 * A non-negative safe integer.
 *
 * Deliberately not `positiveInt` from `./numeric.js`: this admits `0` (a hook
 * may be given no repair turns at all) and the wording it fails with — "a
 * non-negative integer" — is the message this family already emits.
 */
function nonNegativeInt(path: string) {
  const message = `${path} must be a non-negative integer`;
  return z
    .number({ error: message })
    .check(
      z.refine((value: number) => Number.isSafeInteger(value) && value >= 0, {
        error: message,
        abort: true
      })
    );
}

/**
 * Wire shape of a `postHook`, mounted at `path`.
 *
 * Built per call rather than once at module load because the wire path is a
 * parameter of the parser — the same shape is reported as `postHook` on a
 * session config and as `submission.postHook` under a submission, and every
 * message this family emits names the path the caller actually sent.
 *
 * Declaration order is the permitted-key order reported on an unknown field.
 */
export function postHookSchema(path: string) {
  return wireObject(path, {
    command: z.string({ error: `${path}.command must be a string` }),
    timeout: z.optional(
      z.string({
        error: (issue) =>
          `${path}.timeout must be a duration string (e.g. "5m", "30s"); got ${JSON.stringify(issue.input)}`
      })
    ),
    maxTurns: z.optional(nonNegativeInt(`${path}.maxTurns`)),
    maxChars: z.optional(z.nullable(nonNegativeInt(`${path}.maxChars`)))
  });
}

/**
 * The key-and-command gate, without the sibling field rules.
 *
 * A blank command means "no hook", and that answer is reached BEFORE `timeout`,
 * `maxTurns` and `maxChars` are looked at — which is the whole point of the
 * blank-command rule: callers pre-fill a hook config with placeholder values and
 * leave the command empty until they want it to run. Validating the siblings
 * first would reject exactly the configs the rule exists to allow.
 *
 * So the parse happens in two passes: this one decides whether there is a hook
 * at all, and the full {@link postHookSchema} validates the rest only once there
 * is. Both read their keys from the same shape, so there is still one
 * declaration of what a `postHook` may contain.
 */
export function postHookGateSchema(path: string) {
  const { command, ...rest } = postHookSchema(path).def.shape;
  return wireObject(path, {
    command,
    ...Object.fromEntries(Object.keys(rest).map((key) => [key, z.optional(z.unknown())]))
  });
}

/** Validated `postHook` wire input, before normalisation. */
export type PostHookWire = z.infer<ReturnType<typeof postHookSchema>>;
