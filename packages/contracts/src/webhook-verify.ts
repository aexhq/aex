/**
 * Customer-side verification for aex run webhooks (Standard Webhooks scheme).
 *
 * aex signs every delivery HMAC-SHA256 over `${webhook-id}.${webhook-timestamp}.${rawBody}`
 * and sends three headers:
 *   - `webhook-id`         : stable across retries (dedupe key)
 *   - `webhook-timestamp`  : unix seconds at send time
 *   - `webhook-signature`  : space-delimited list of `v1,<base64(hmac)>` entries
 *                            (a list during secret rotation — accept ANY match)
 *
 * The secret is the workspace signing secret surfaced once as `whsec_<base64>`;
 * the HMAC key is the raw bytes after the `whsec_` prefix. Verification accepts
 * the secret with or without the prefix.
 *
 * Pure Web Crypto — identical under Bun and Node; this mirrors the
 * `standardwebhooks` library so a customer can verify with either.
 */

const encoder = new TextEncoder();

export interface VerifyAexWebhookInput {
  /** The exact bytes of the request body, as a string (do NOT re-serialize). */
  readonly rawBody: string;
  /** The inbound request headers (case-insensitive lookup). */
  readonly headers:
    | Headers
    | Readonly<Record<string, string | string[] | undefined>>;
  /** The workspace signing secret, `whsec_<base64>` (or the bare base64). */
  readonly secret: string;
  /** Max allowed clock skew between `webhook-timestamp` and now. Default 300s. */
  readonly toleranceSeconds?: number;
}

function headerValue(
  headers: VerifyAexWebhookInput["headers"],
  name: string
): string | undefined {
  if (headers instanceof Headers) {
    return headers.get(name) ?? undefined;
  }
  const lower = name.toLowerCase();
  for (const [key, value] of Object.entries(headers)) {
    if (key.toLowerCase() === lower) {
      return Array.isArray(value) ? value[0] : value ?? undefined;
    }
  }
  return undefined;
}

/** Strip the `whsec_` prefix and decode the base64 key material to bytes. */
function decodeSecret(secret: string): Uint8Array {
  const material = secret.startsWith("whsec_") ? secret.slice("whsec_".length) : secret;
  const binary = atob(material);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

async function hmacBase64(keyBytes: Uint8Array, message: string): Promise<string> {
  const key = await crypto.subtle.importKey(
    "raw",
    keyBytes as unknown as ArrayBuffer,
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"]
  );
  const sig = await crypto.subtle.sign("HMAC", key, encoder.encode(message));
  let binary = "";
  const view = new Uint8Array(sig);
  for (let i = 0; i < view.length; i++) binary += String.fromCharCode(view[i]!);
  return btoa(binary);
}

/** Length-then-constant-time comparison of two strings. */
function timingSafeEqual(a: string, b: string): boolean {
  if (a.length !== b.length) return false;
  let diff = 0;
  for (let i = 0; i < a.length; i++) diff |= a.charCodeAt(i) ^ b.charCodeAt(i);
  return diff === 0;
}

/**
 * Verify an aex webhook delivery. Returns `true` only when the timestamp is
 * within tolerance AND one of the `v1,<base64>` entries in `webhook-signature`
 * matches the HMAC over `${id}.${timestamp}.${rawBody}`. Constant-time compare;
 * never throws on a bad signature (only on a malformed secret).
 */
export async function verifyAexWebhook(input: VerifyAexWebhookInput): Promise<boolean> {
  const toleranceSeconds = input.toleranceSeconds ?? 300;
  const id = headerValue(input.headers, "webhook-id");
  const timestamp = headerValue(input.headers, "webhook-timestamp");
  const signatureHeader = headerValue(input.headers, "webhook-signature");
  if (!id || !timestamp || !signatureHeader) return false;

  const ts = Number(timestamp);
  if (!Number.isFinite(ts)) return false;
  const nowSeconds = Math.floor(Date.now() / 1000);
  if (Math.abs(nowSeconds - ts) > toleranceSeconds) return false;

  const keyBytes = decodeSecret(input.secret);
  const expected = await hmacBase64(keyBytes, `${id}.${timestamp}.${input.rawBody}`);

  // `webhook-signature` is a space-delimited list of versioned signatures; a
  // single delivery may carry several (current + previous key during rotation).
  for (const entry of signatureHeader.split(" ")) {
    const comma = entry.indexOf(",");
    if (comma === -1) continue;
    const version = entry.slice(0, comma);
    const value = entry.slice(comma + 1);
    if (version !== "v1") continue;
    if (timingSafeEqual(value, expected)) return true;
  }
  return false;
}
