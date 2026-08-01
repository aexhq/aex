import { verifyCsrf } from "../../../src/server/csrf";
import { readCookie } from "../../../src/server/passthrough";

export const dynamic = "force-dynamic";

const PRIVATE = { "Cache-Control": "private, no-store" } as const;

/**
 * TODO(cross-stream): sign-out must revoke centrally through
 * `DELETE /internal/v1/identity/sessions` before the cookie is cleared. Central
 * identity has not published that endpoint in this branch, so this handler clears
 * the browser's copy only and the credential remains valid until it expires.
 */
export function GET(request: Request): Response {
  const present = readCookie(request.headers.get("cookie"), "__Host-aex_session") !== null;
  return Response.json({ authenticated: present }, { headers: PRIVATE });
}

export function DELETE(request: Request): Response {
  const cookie = readCookie(request.headers.get("cookie"), "__Host-aex_csrf");
  const header = request.headers.get("x-aex-csrf");
  const fetchSite = request.headers.get("sec-fetch-site");
  const proven = verifyCsrf({
    ...(cookie === null ? {} : { cookie }),
    ...(header === null ? {} : { header }),
    ...(fetchSite === null ? {} : { fetchSite }),
  });
  if (!proven) {
    return Response.json(
      { error: { code: "forbidden", message: "cross-site or unproven request", retryable: false } },
      { status: 403, headers: PRIVATE },
    );
  }
  return new Response(null, {
    status: 204,
    headers: {
      ...PRIVATE,
      "Set-Cookie": "__Host-aex_session=; Max-Age=0; Path=/; HttpOnly; Secure; SameSite=Lax",
    },
  });
}
