import { verifyCsrf } from "../../../src/server/csrf";
import { errorResponse, readCookie } from "../../../src/server/passthrough";
import { CLEARED_CSRF_COOKIE, CLEARED_SESSION_COOKIE } from "../../../src/server/session";
import { closeDashboardSession } from "../../../src/server/signin";

export const dynamic = "force-dynamic";

const PRIVATE = { "Cache-Control": "private, no-store" } as const;

export function GET(request: Request): Response {
  const present = readCookie(request.headers.get("cookie"), "__Host-aex_session") !== null;
  return Response.json({ authenticated: present }, { headers: PRIVATE });
}

export async function DELETE(request: Request): Promise<Response> {
  const cookies = request.headers.get("cookie");
  const cookie = readCookie(cookies, "__Host-aex_csrf");
  const header = request.headers.get("x-aex-csrf");
  const fetchSite = request.headers.get("sec-fetch-site");
  const proven = verifyCsrf({
    ...(cookie === null ? {} : { cookie }),
    ...(header === null ? {} : { header }),
    ...(fetchSite === null ? {} : { fetchSite }),
  });
  if (!proven) {
    return errorResponse(403, "forbidden", "cross-site or unproven request");
  }

  const credential = readCookie(cookies, "__Host-aex_session");
  if (credential !== null) {
    const outcome = await closeDashboardSession(credential);
    if (outcome.kind === "failed") {
      return errorResponse(outcome.status, outcome.code, outcome.message);
    }
  }

  const headers = new Headers(PRIVATE);
  headers.append("Set-Cookie", CLEARED_SESSION_COOKIE);
  headers.append("Set-Cookie", CLEARED_CSRF_COOKIE);
  return new Response(null, { status: 204, headers });
}
