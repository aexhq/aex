import { equalsConstantTime } from "../../../../src/server/csrf";
import { readCookie } from "../../../../src/server/passthrough";
import { signInPath } from "../../../../src/server/return-to";
import {
  CLEARED_SIGN_IN_STATE_COOKIE,
  csrfCookie,
  dashboardSessionCookie,
} from "../../../../src/server/session";
import {
  STATE_COOKIE,
  SignInError,
  challengeOf,
  decodeSignInState,
  isFreshState,
  mintToken,
  openDashboardSession,
  sessionLifetimeSeconds,
  type SignInFailure,
} from "../../../../src/server/signin";

export const dynamic = "force-dynamic";

const PRIVATE = { "Cache-Control": "private, no-store" } as const;

function refuse(failure: SignInFailure, returnTo: string, clearBinding: boolean): Response {
  const target = signInPath(returnTo);
  const separator = target.includes("?") ? "&" : "?";
  const headers = new Headers({ ...PRIVATE, location: `${target}${separator}error=${failure}` });
  if (clearBinding) headers.append("Set-Cookie", CLEARED_SIGN_IN_STATE_COOKIE);
  return new Response(null, { status: 303, headers });
}

export async function GET(request: Request): Promise<Response> {
  const binding = decodeSignInState(readCookie(request.headers.get("cookie"), STATE_COOKIE));
  const returnTo = binding?.returnTo ?? "/";
  const url = new URL(request.url);
  const presented = url.searchParams.get("state");

  if (
    binding === null ||
    presented === null ||
    !isFreshState(binding) ||
    !equalsConstantTime(await challengeOf(binding.verifier), presented)
  ) {
    return refuse("binding_failed", returnTo, false);
  }

  if (url.searchParams.get("error") !== null) {
    return refuse("provider_denied", returnTo, true);
  }
  const code = url.searchParams.get("code");
  if (code === null) return refuse("binding_failed", returnTo, true);

  try {
    const credential = await openDashboardSession({
      provider: binding.provider,
      code,
      state: presented,
      codeVerifier: binding.verifier,
    });
    const lifetime = sessionLifetimeSeconds(credential.expiresAt);
    const headers = new Headers({ ...PRIVATE, location: returnTo });
    headers.append("Set-Cookie", dashboardSessionCookie(credential.session, lifetime));
    headers.append("Set-Cookie", csrfCookie(mintToken(), lifetime));
    headers.append("Set-Cookie", CLEARED_SIGN_IN_STATE_COOKIE);
    return new Response(null, { status: 303, headers });
  } catch (error) {
    if (error instanceof SignInError) return refuse(error.failure, returnTo, true);
    throw error;
  }
}
