/** Canonical, redaction-safe terminal fault reported by an upstream provider. */

export const PROVIDER_FAULT_KINDS = [
  "rate_limit",
  "overloaded",
  "quota_exceeded",
  "unavailable",
  "provider_error"
] as const;

export type KnownProviderFaultKind = (typeof PROVIDER_FAULT_KINDS)[number];

declare const UNKNOWN_PROVIDER_FAULT_KIND: unique symbol;

/**
 * A syntactically valid future provider-fault kind accepted from the wire.
 * Producers must use a known kind; only the canonical parser creates this type.
 */
export type UnknownProviderFaultKind = string & {
  readonly [UNKNOWN_PROVIDER_FAULT_KIND]: "UnknownProviderFaultKind";
};

export type ProviderFaultKind = KnownProviderFaultKind | UnknownProviderFaultKind;

export interface ProviderFault {
  /** Upstream provider id, for example `anthropic`, when reported. */
  readonly provider?: string;
  /** Exact machine-readable fault class. Unknown future tokens are non-throttling. */
  readonly kind: ProviderFaultKind;
  /** Upstream HTTP status, when reported. */
  readonly status?: number;
  /** Retry delay in milliseconds, when reported. */
  readonly retryAfterMs?: number;
  /** Short, already-redacted diagnostic message. */
  readonly message?: string;
}

const PROVIDER_FAULT_KIND_SET = new Set<string>(PROVIDER_FAULT_KINDS);
const PROVIDER_FAULT_KEYS = new Set(["provider", "kind", "status", "retryAfterMs", "message"]);
const KIND_TOKEN = /^[a-z][a-z0-9_.:-]{0,63}$/;
const PROVIDER_TOKEN = /^[A-Za-z0-9][A-Za-z0-9_.:-]{0,127}$/;
const MAX_MESSAGE_LENGTH = 512;

export function isKnownProviderFaultKind(kind: string): kind is KnownProviderFaultKind {
  return PROVIDER_FAULT_KIND_SET.has(kind);
}

/**
 * Strictly parse the canonical provider-fault wire object.
 *
 * This parser intentionally rejects aliases, coercions, unknown fields, and
 * malformed present values. A valid future `kind` token is preserved for
 * forward compatibility, but callers must not infer throttle semantics from it.
 */
export function parseProviderFault(value: unknown): ProviderFault {
  if (!isRecord(value)) throw new TypeError("providerFault must be an object");
  for (const key of Object.keys(value)) {
    if (!PROVIDER_FAULT_KEYS.has(key)) {
      throw new TypeError("providerFault contains an unknown field");
    }
  }
  if (!Object.hasOwn(value, "kind") || typeof value.kind !== "string" || !KIND_TOKEN.test(value.kind)) {
    throw new TypeError("providerFault.kind must be a canonical token");
  }
  if (Object.hasOwn(value, "provider") && (
    typeof value.provider !== "string" || !PROVIDER_TOKEN.test(value.provider)
  )) {
    throw new TypeError("providerFault.provider must be a canonical provider token");
  }
  if (Object.hasOwn(value, "status") && (
    !Number.isInteger(value.status) || (value.status as number) < 100 || (value.status as number) > 599
  )) {
    throw new TypeError("providerFault.status must be an HTTP status integer");
  }
  if (Object.hasOwn(value, "retryAfterMs") && (
    !Number.isSafeInteger(value.retryAfterMs) || (value.retryAfterMs as number) < 0
  )) {
    throw new TypeError("providerFault.retryAfterMs must be a non-negative safe integer");
  }
  if (Object.hasOwn(value, "message") && (
    typeof value.message !== "string" || value.message.length === 0 ||
    value.message.length > MAX_MESSAGE_LENGTH || /[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/.test(value.message)
  )) {
    throw new TypeError(`providerFault.message must be 1-${MAX_MESSAGE_LENGTH} printable characters`);
  }

  const kind = isKnownProviderFaultKind(value.kind)
    ? value.kind
    : value.kind as UnknownProviderFaultKind;
  return {
    kind,
    ...(Object.hasOwn(value, "provider") ? { provider: value.provider as string } : {}),
    ...(Object.hasOwn(value, "status") ? { status: value.status as number } : {}),
    ...(Object.hasOwn(value, "retryAfterMs") ? { retryAfterMs: value.retryAfterMs as number } : {}),
    ...(Object.hasOwn(value, "message") ? { message: value.message as string } : {})
  };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
