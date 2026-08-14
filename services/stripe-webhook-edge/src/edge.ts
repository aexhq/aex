import { HANDLED_EVENT_TYPES, STRIPE_API_VERSION } from "./protocol.js";

export { HANDLED_EVENT_TYPES };

export const MAX_BODY_BYTES = 262_144;

export interface RawRequest {
  readonly body: string | null;
  readonly isBase64Encoded?: boolean;
}

export function decodeRawBody(request: RawRequest): Buffer {
  if (request.body === null) throw new Error("webhook body is missing");
  const body = request.isBase64Encoded
    ? Buffer.from(request.body, "base64")
    : Buffer.from(request.body, "utf8");
  if (body.byteLength > MAX_BODY_BYTES) {
    throw new Error(`webhook body exceeds ${MAX_BODY_BYTES} bytes`);
  }
  return body;
}

interface SignedEvent {
  readonly id: string;
  readonly type: string;
  readonly api_version?: string | null;
}

type Verifier<T extends SignedEvent> = (body: Buffer, signature: string, secret: string) => T;

/** Tries the bounded rotation set in order and accepts no third secret. */
export function verifyWithRotatingSecrets<T extends SignedEvent>(
  body: Buffer,
  signature: string,
  secrets: readonly string[],
  verifier: Verifier<T>,
): T {
  if (secrets.length === 0 || secrets.length > 2) {
    throw new Error("webhook secret set must contain one or two secrets");
  }
  let failure: unknown = new Error("webhook signature verification failed");
  for (const secret of secrets) {
    try {
      return verifier(body, signature, secret);
    } catch (error: unknown) {
      failure = error;
    }
  }
  throw failure;
}

export type EventDisposition =
  | { readonly disposition: "forward"; readonly providerEventId: string; readonly eventType: string }
  | { readonly disposition: "ignored_unsupported"; readonly providerEventId: string }
  | { readonly disposition: "quarantined_api_version"; readonly providerEventId: string };

/** Applies version fencing before the event can reach finance. */
export function normalizeEvent(event: SignedEvent): EventDisposition {
  if (event.api_version !== STRIPE_API_VERSION) {
    return { disposition: "quarantined_api_version", providerEventId: event.id };
  }
  if (!(HANDLED_EVENT_TYPES as readonly string[]).includes(event.type)) {
    return { disposition: "ignored_unsupported", providerEventId: event.id };
  }
  return { disposition: "forward", providerEventId: event.id, eventType: event.type };
}
