import { NextResponse, type NextRequest } from "next/server";

import { RETURN_HEADER, safeReturnPath } from "./src/server/return-to";

const SESSION_COOKIE = "__Host-aex_session";

function isPublicPage(pathname: string): boolean {
  return pathname === "/" || pathname === "/og.png" || pathname === "/docs" || pathname.startsWith("/docs/");
}

/** Preserve the exact same-origin destination across the browser sign-in bounce. */
export default function proxy(request: NextRequest): NextResponse {
  if (isPublicPage(request.nextUrl.pathname)) return NextResponse.next();

  const target = `${request.nextUrl.pathname}${request.nextUrl.search}`;
  if (request.cookies.has(SESSION_COOKIE)) {
    const headers = new Headers(request.headers);
    headers.set(RETURN_HEADER, safeReturnPath(target));
    return NextResponse.next({ request: { headers } });
  }

  const url = request.nextUrl.clone();
  url.pathname = "/signin";
  url.search = "";
  const safe = safeReturnPath(target);
  if (safe !== "/") url.searchParams.set("next", safe);
  return NextResponse.redirect(url);
}

export const config = {
  matcher: ["/((?!api/|_next/static|_next/image|signin|favicon.ico|og.png).*)"],
};
