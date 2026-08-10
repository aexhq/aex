import { safeReturnPath } from "../../../../../src/server/return-to";
import { signInStateCookie } from "../../../../../src/server/session";
import {
  STATE_TTL_MS,
  authorizationUrl,
  callbackUrl,
  challengeOf,
  encodeSignInState,
  isProviderId,
  mintToken,
  signInConfig,
} from "../../../../../src/server/signin";

export const dynamic = "force-dynamic";

interface Context {
  readonly params: Promise<{ readonly provider: string }>;
}

export async function GET(request: Request, context: Context): Promise<Response> {
  const { provider } = await context.params;
  if (!isProviderId(provider)) return new Response(null, { status: 404 });

  const config = signInConfig();
  if (config.kind !== "ready") return new Response(null, { status: 404 });
  const clientId = config.clientIds[provider];
  if (clientId === undefined) return new Response(null, { status: 404 });

  const verifier = mintToken();
  const challenge = await challengeOf(verifier);
  const returnTo = safeReturnPath(new URL(request.url).searchParams.get("next"));
  const redirectUri = callbackUrl(config.origin);
  const state = encodeSignInState({
    provider,
    verifier,
    returnTo,
    issuedAt: Date.now(),
  });

  return new Response(null, {
    status: 303,
    headers: {
      location: authorizationUrl(provider, clientId, redirectUri, challenge),
      "Cache-Control": "private, no-store",
      "Set-Cookie": signInStateCookie(state, Math.floor(STATE_TTL_MS / 1_000)),
    },
  });
}
