import {
  AexApiError,
  apiErrorFromResponse,
  type RegionCode,
  type RouteDescriptor,
  type WireRequest,
} from "@aexhq/sdk";

import { authorizeDashboardRoute } from "./routes";
import { verifyCsrf } from "./csrf";
import { CLIENT_HEADER, isRegionCode, transportFor } from "./upstream";

/** Upstream deadline. A panel's own deadline is shorter; this is the backstop. */
export const UPSTREAM_TIMEOUT_MS = 10_000;

/** A forwarded body is bounded before it is parsed, not after. */
export const MAX_BODY_BYTES = 64 * 1024;

/** Path and query values are opaque identifiers, never paths or URLs. */
const SAFE_PARAMETER = /^[A-Za-z0-9._:-]{1,512}$/;

const SESSION_COOKIE = "__Host-aex_session";
const CSRF_COOKIE = "__Host-aex_csrf";
const CSRF_HEADER = "x-aex-csrf";

export interface Refusal {
  readonly status: number;
  readonly code: string;
  readonly message: string;
}

export type Resolution =
  | { readonly ok: true; readonly request: WireRequest; readonly descriptor: RouteDescriptor; readonly region: RegionCode | null }
  | { readonly ok: false; readonly refusal: Refusal };

function refuse(status: number, code: string, message: string): Resolution {
  return { ok: false, refusal: { status, code, message } };
}

export function readCookie(header: string | null, name: string): string | null {
  if (!header) return null;
  for (const part of header.split(";")) {
    const separator = part.indexOf("=");
    if (separator < 0) continue;
    if (part.slice(0, separator).trim() !== name) continue;
    try {
      return decodeURIComponent(part.slice(separator + 1).trim());
    } catch {
      return null;
    }
  }
  return null;
}

/**
 * Resolve a browser request into exactly one wire request, or refuse it.
 *
 * Order matters and is asserted by `test/passthrough.test.ts`: the route allowlist
 * and the method are checked before the session cookie is read, so an off-allowlist
 * probe never reaches the upstream and never sees a credential attached.
 */
export function resolvePassthrough(
  plane: string,
  routeId: string,
  request: Request,
  body: Uint8Array | null,
): Resolution {
  const descriptor = authorizeDashboardRoute(routeId);
  if (!descriptor) return refuse(404, "not_found", "unknown operation");
  if (descriptor.plane !== plane) return refuse(404, "not_found", "unknown operation");
  if (request.method !== descriptor.method) {
    return refuse(405, "invalid_request", "the operation does not accept this method");
  }

  if (request.method !== "GET") {
    const cookies = request.headers.get("cookie");
    const csrf = readCookie(cookies, CSRF_COOKIE);
    const presented = request.headers.get(CSRF_HEADER);
    const fetchSite = request.headers.get("sec-fetch-site");
    const input = {
      ...(csrf === null ? {} : { cookie: csrf }),
      ...(presented === null ? {} : { header: presented }),
      ...(fetchSite === null ? {} : { fetchSite }),
    };
    if (!verifyCsrf(input)) return refuse(403, "forbidden", "cross-site or unproven request");
  }

  const credential = readCookie(request.headers.get("cookie"), SESSION_COOKIE);
  if (!credential) return refuse(401, "unauthenticated", "browser session required");

  const url = new URL(request.url);
  const supplied = new Set(url.searchParams.keys());
  supplied.delete("region");

  let region: RegionCode | null = null;
  if (descriptor.plane === "regional") {
    const value = url.searchParams.get("region");
    if (!value || !isRegionCode(value)) return refuse(400, "invalid_request", "a valid region is required");
    region = value;
  }

  let path = descriptor.path;
  for (const name of descriptor.pathParams) {
    const value = url.searchParams.get(name);
    if (!value || !SAFE_PARAMETER.test(value)) {
      return refuse(400, "invalid_request", `the path parameter ${name} is missing or malformed`);
    }
    path = path.replace(`{${name}}`, encodeURIComponent(value));
    supplied.delete(name);
  }

  const forwarded = new URLSearchParams();
  for (const name of descriptor.queryParams) {
    const value = url.searchParams.get(name);
    if (value === null) continue;
    if (!SAFE_PARAMETER.test(value)) {
      return refuse(400, "invalid_request", `the query parameter ${name} is malformed`);
    }
    forwarded.set(name, value);
    supplied.delete(name);
  }
  if (supplied.size > 0) {
    return refuse(400, "invalid_request", "the operation does not accept every supplied parameter");
  }
  const query = forwarded.toString();

  const headers = new Headers({
    authorization: `Bearer ${credential}`,
    accept: "application/json",
    "Aex-Client": CLIENT_HEADER,
  });

  if (descriptor.idempotency === "idempotency_key") {
    const key = request.headers.get("idempotency-key");
    if (!key || !SAFE_PARAMETER.test(key)) {
      return refuse(400, "invalid_request", "this operation requires an Idempotency-Key header");
    }
    headers.set("Idempotency-Key", key);
  }

  let bytes: Uint8Array | undefined;
  if (body !== null && body.byteLength > 0) {
    if (body.byteLength > MAX_BODY_BYTES) return refuse(413, "payload_too_large", "request body too large");
    let parsed: unknown;
    try {
      parsed = JSON.parse(new TextDecoder().decode(body));
    } catch {
      return refuse(400, "invalid_request", "request body is not JSON");
    }
    headers.set("content-type", "application/json");
    bytes = new TextEncoder().encode(JSON.stringify(parsed));
  }

  return {
    ok: true,
    descriptor,
    region,
    request: {
      routeId: descriptor.id,
      method: descriptor.method,
      path: query ? `${path}?${query}` : path,
      headers,
      ...(bytes ? { body: bytes } : {}),
    },
  };
}

/** The redacted envelope the browser sees. Upstream prose never reaches it raw. */
export function errorResponse(status: number, code: string, message: string, requestId?: string): Response {
  return Response.json(
    {
      error: {
        code,
        message,
        retryable: status === 429 || status === 502 || status === 503 || status === 504,
        ...(requestId ? { requestId } : {}),
      },
    },
    { status, headers: { "Cache-Control": "private, no-store" } },
  );
}

export async function executePassthrough(resolution: Resolution): Promise<Response> {
  if (!resolution.ok) {
    const { status, code, message } = resolution.refusal;
    return errorResponse(status, code, message);
  }
  const transport = transportFor(resolution.descriptor.plane, resolution.region);
  const signal = AbortSignal.timeout(UPSTREAM_TIMEOUT_MS);
  let response;
  try {
    response = await transport.execute<unknown>({ ...resolution.request, signal });
  } catch {
    return errorResponse(504, "upstream_error", "the upstream did not answer in time");
  }
  if (response.status >= 400) {
    // Classification and message redaction both come from the SDK; the BFF adds none.
    const error: AexApiError = apiErrorFromResponse(
      resolution.descriptor.id,
      response.status,
      response.body,
      response.headers,
    );
    const retryAfter = response.headers.get("retry-after");
    return Response.json(
      {
        error: {
          code: error.code,
          message: error.message,
          requestId: error.requestId,
          retryable: error.retryable,
          ...(error.operationId ? { operationId: error.operationId } : {}),
          ...(error.details === undefined ? {} : { details: error.details }),
        },
      },
      {
        status: response.status,
        headers: {
          "Cache-Control": "private, no-store",
          ...(retryAfter ? { "Retry-After": retryAfter } : {}),
        },
      },
    );
  }
  const requestId = response.headers.get("x-request-id");
  return Response.json(response.body, {
    status: response.status,
    headers: {
      "Cache-Control": "private, no-store",
      ...(requestId ? { "X-Request-Id": requestId } : {}),
    },
  });
}
