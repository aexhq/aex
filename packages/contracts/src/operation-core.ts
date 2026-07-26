/**
 * The two primitives every client operation module needs, and the only two that
 * would otherwise have to be duplicated or imported across a cycle: the
 * client-side config-rejection factory, and the idempotency-key policy.
 *
 * A LEAF on purpose. It imports `ids.js` and `sdk-errors.js` and nothing else,
 * so `operations.ts` and `account-operations.ts` can each depend on it without
 * depending on each other. The public names below are re-exported from
 * `operations.ts`, which is where callers have always found them.
 */
import { createHash } from "node:crypto";
import { newId } from "./ids.js";
import { SessionConfigValidationError } from "./sdk-errors.js";

export interface IdempotencyOptions {
  readonly idempotencyKey?: string;
}

export const IDEMPOTENCY_KEY_MAX_LENGTH = 255;
const MESSAGE_IDEMPOTENCY_SUFFIX = ":message";

/**
 * The factory for every public client-side session config rejection.
 *
 * PACKAGE-private, not public: `operations.ts` and `account-operations.ts`
 * import it, and neither re-exports it, so it stays off the `operations`
 * namespace exactly as it was when it was a `function` local to `operations.ts`.
 */
export function configError(field: string, message: string): SessionConfigValidationError {
  return new SessionConfigValidationError(message, { field });
}

/**
 * Resolve a caller-supplied idempotency key to the value that ships on the
 * request. FAIL-FAST: an empty or whitespace-only key THROWS
 * {@link SessionConfigValidationError} — a footgun that silently disabled dedup
 * (`?? generate()` kept `''`, then a downstream truthy header-drop shipped no
 * `Idempotency-Key`). An absent key generates a fresh one; a real key is
 * returned verbatim. The single choke point every send/create/run entry uses.
 */
export function resolveIdempotencyKey(key?: string): string {
  if (key === undefined) {
    return newId("idempotency");
  }
  if (typeof key !== "string" || key.trim().length === 0) {
    throw configError("idempotencyKey", "idempotencyKey must be a non-empty, non-whitespace string");
  }
  if (key.length > IDEMPOTENCY_KEY_MAX_LENGTH) {
    throw configError("idempotencyKey", `idempotencyKey must be at most ${IDEMPOTENCY_KEY_MAX_LENGTH} characters`);
  }
  return key;
}

/**
 * Derive the first-message identity from a session-create identity without
 * crossing the hosted 255-character header limit. Short keys retain the
 * readable `<createKey>:message` form; long keys use a deterministic digest.
 */
export function deriveMessageIdempotencyKey(createKey: string): string {
  const validated = resolveIdempotencyKey(createKey);
  const readable = `${validated}${MESSAGE_IDEMPOTENCY_SUFFIX}`;
  if (readable.length <= IDEMPOTENCY_KEY_MAX_LENGTH) return readable;
  const digest = createHash("sha256").update(validated, "utf8").digest("hex");
  return `aex-message-sha256-${digest}`;
}

/**
 * Fail-closed idempotency header builder. An EMPTY string throws (defense in
 * depth alongside {@link resolveIdempotencyKey}) rather than silently dropping
 * the header and proceeding non-idempotent; an absent key yields no header.
 *
 * Package-private on the same terms as {@link configError}.
 */
export function idempotencyHeaders(options?: IdempotencyOptions): HeadersInit | undefined {
  if (options?.idempotencyKey === undefined) return undefined;
  return { "Idempotency-Key": resolveIdempotencyKey(options.idempotencyKey) };
}
