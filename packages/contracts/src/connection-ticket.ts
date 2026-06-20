/**
 * Connection tickets for the event-coordinator WebSocket handshake.
 *
 * A browser/runtime WebSocket handshake cannot carry an Authorization header,
 * so a subscriber first obtains a short-lived ticket from an authenticated
 * HTTP endpoint, then presents it as a `?ticket=` query parameter on the WS
 * upgrade. The ticket is an HMAC over `${runId}.${channel}.${exp}` keyed by
 * the coordinator secret — it binds the grant to one run, one stream channel,
 * and one expiry and is verified without any per-ticket storage.
 *
 * This lives in shared so the coordinator (which verifies) and the API
 * hosted API's ticket broker (which mints, on behalf of a workspace token) use
 * ONE implementation. Pure Web Crypto — identical under Bun, Node, and workerd.
 */

const DEFAULT_TICKET_TTL_MS = 60_000;
const encoder = new TextEncoder();

export type ConnectionTicketChannel = "event" | "log" | "all";

function normalizeTicketChannel(channel: ConnectionTicketChannel | undefined): ConnectionTicketChannel {
  return channel === "log" || channel === "all" ? channel : "event";
}

async function hmacHex(secret: string, message: string): Promise<string> {
  const key = await crypto.subtle.importKey(
    "raw",
    encoder.encode(secret),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"]
  );
  const sig = await crypto.subtle.sign("HMAC", key, encoder.encode(message));
  return [...new Uint8Array(sig)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

/** Mint a `${channel}.${exp}.${mac}` ticket valid for `ttlMs` from `nowMs`. */
export async function mintConnectionTicket(
  runId: string,
  secret: string,
  nowMs: number,
  ttlMs: number = DEFAULT_TICKET_TTL_MS,
  channel: ConnectionTicketChannel = "event"
): Promise<{ ticket: string; expiresAtMs: number }> {
  const boundChannel = normalizeTicketChannel(channel);
  const exp = nowMs + ttlMs;
  const mac = await hmacHex(secret, `${runId}.${boundChannel}.${exp}`);
  return { ticket: `${boundChannel}.${exp}.${mac}`, expiresAtMs: exp };
}

/** Verify a ticket against `runId`, channel, and current time. Constant-time MAC compare. */
export async function verifyConnectionTicket(
  ticket: string,
  runId: string,
  secret: string,
  nowMs: number,
  channel: ConnectionTicketChannel = "event"
): Promise<boolean> {
  const [ticketChannel, expRaw, mac, ...rest] = ticket.split(".");
  if (rest.length > 0 || !ticketChannel || !expRaw || !mac) return false;
  const boundChannel = normalizeTicketChannel(channel);
  if (ticketChannel !== boundChannel) return false;
  const exp = Number(expRaw);
  if (!Number.isFinite(exp) || exp < nowMs) return false;
  const expected = await hmacHex(secret, `${runId}.${boundChannel}.${exp}`);
  return timingSafeEqual(mac, expected);
}

/** Length-then-constant-time comparison of two hex strings. */
function timingSafeEqual(a: string, b: string): boolean {
  if (a.length !== b.length) return false;
  let diff = 0;
  for (let i = 0; i < a.length; i++) diff |= a.charCodeAt(i) ^ b.charCodeAt(i);
  return diff === 0;
}
