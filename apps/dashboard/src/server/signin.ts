import {
  ROUTES,
  apiErrorFromResponse,
  type AexTransport,
  type WireRequest,
} from "@aexhq/sdk";
import {
  DashboardSessionCredentialSchema,
  DashboardSessionRequestSchema,
  type DashboardSessionCredential,
  type DashboardSessionRequest,
} from "@aexhq/wire";

import { safeReturnPath } from "./return-to";
import { MAX_SESSION_SECONDS } from "./session";
import { CENTRAL_URL_KEY, CLIENT_HEADER, centralBaseUrl, transportFor } from "./upstream";

export const PROVIDERS = ["google"] as const;
export type ProviderId = (typeof PROVIDERS)[number];

export const PROVIDER_LABEL: Readonly<Record<ProviderId, string>> = {
  google: "Google",
};

export function isProviderId(value: string): value is ProviderId {
  return (PROVIDERS as readonly string[]).includes(value);
}

export const SIGN_IN_FAILURES = [
  "binding_failed",
  "provider_denied",
  "exchange_refused",
  "exchange_unavailable",
] as const;
export type SignInFailure = (typeof SIGN_IN_FAILURES)[number];

export function isSignInFailure(value: string): value is SignInFailure {
  return (SIGN_IN_FAILURES as readonly string[]).includes(value);
}

export class SignInError extends Error {
  readonly failure: SignInFailure;

  constructor(failure: SignInFailure, detail: string) {
    super(`${failure}: ${detail}`);
    this.name = "SignInError";
    this.failure = failure;
  }
}

export const CONFIG_KEYS = {
  origin: "AEX_DASHBOARD_ORIGIN",
  central: CENTRAL_URL_KEY,
  googleClientId: "AEX_OAUTH_GOOGLE_CLIENT_ID",
} as const;

export type Environment = Readonly<Record<string, string | undefined>>;

export type SignInConfig =
  | { readonly kind: "unconfigured" }
  | {
      readonly kind: "ready";
      readonly origin: string;
      readonly central: string;
      readonly clientIds: Readonly<Record<ProviderId, string>>;
    };

function present(environment: Environment, key: string): string | null {
  const value = environment[key]?.trim();
  return value ? value : null;
}

function originOf(environment: Environment): string {
  const raw = present(environment, CONFIG_KEYS.origin);
  if (raw === null) {
    throw new Error(`${CONFIG_KEYS.origin} is required once a sign-in provider is configured`);
  }
  let url: URL;
  try {
    url = new URL(raw);
  } catch {
    throw new Error(`${CONFIG_KEYS.origin} must be an absolute URL`);
  }
  if (
    url.protocol !== "https:" ||
    url.pathname !== "/" ||
    url.search ||
    url.hash ||
    url.username ||
    url.password
  ) {
    throw new Error(`${CONFIG_KEYS.origin} must be a bare HTTPS origin`);
  }
  return url.origin;
}

export function signInConfig(environment: Environment = process.env): SignInConfig {
  const origin = present(environment, CONFIG_KEYS.origin);
  const central = present(environment, CONFIG_KEYS.central);
  const googleClientId = present(environment, CONFIG_KEYS.googleClientId);
  if (origin === null && central === null && googleClientId === null) {
    return { kind: "unconfigured" };
  }
  if (central === null) {
    throw new Error(`${CONFIG_KEYS.central} is required once sign-in is configured`);
  }
  if (googleClientId === null) {
    throw new Error(`${CONFIG_KEYS.googleClientId} is required once sign-in is configured`);
  }
  return {
    kind: "ready",
    origin: originOf(environment),
    central: centralBaseUrl(environment),
    clientIds: { google: googleClientId },
  };
}

export function configuredProviders(environment: Environment = process.env): readonly ProviderId[] {
  const config = signInConfig(environment);
  return config.kind === "ready" ? PROVIDERS : [];
}

export function callbackUrl(origin: string): string {
  return new URL("/api/auth/callback", origin).toString();
}

export function authorizationUrl(
  provider: ProviderId,
  clientId: string,
  redirectUri: string,
  challenge: string,
): string {
  const url = new URL("https://accounts.google.com/o/oauth2/v2/auth");
  url.searchParams.set("client_id", clientId);
  url.searchParams.set("redirect_uri", redirectUri);
  url.searchParams.set("response_type", "code");
  url.searchParams.set("scope", "openid email profile");
  url.searchParams.set("state", challenge);
  url.searchParams.set("code_challenge", challenge);
  url.searchParams.set("code_challenge_method", "S256");
  url.searchParams.set("nonce", challenge);
  return url.toString();
}

function base64url(bytes: Uint8Array): string {
  return Buffer.from(bytes).toString("base64url");
}

export function mintToken(): string {
  return base64url(crypto.getRandomValues(new Uint8Array(32)));
}

export async function challengeOf(verifier: string): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(verifier));
  return base64url(new Uint8Array(digest));
}

export const STATE_COOKIE = "__Host-aex_signin";
export const STATE_TTL_MS = 600_000;
const CLOCK_SKEW_MS = 1_000;
const VERIFIER = /^[A-Za-z0-9._~-]{43,128}$/;
const COOKIE = /^[A-Za-z0-9_-]{1,2048}$/;

export interface SignInState {
  readonly provider: ProviderId;
  readonly verifier: string;
  readonly returnTo: string;
  readonly issuedAt: number;
}

export function encodeSignInState(state: SignInState): string {
  return Buffer.from(JSON.stringify(state)).toString("base64url");
}

export function decodeSignInState(value: string | null): SignInState | null {
  if (value === null || !COOKIE.test(value)) return null;
  let parsed: unknown;
  try {
    parsed = JSON.parse(Buffer.from(value, "base64url").toString("utf8"));
  } catch {
    return null;
  }
  if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) return null;
  const record = parsed as Record<string, unknown>;
  if (
    Object.keys(record).sort().join(",") !== "issuedAt,provider,returnTo,verifier" ||
    typeof record["provider"] !== "string" ||
    !isProviderId(record["provider"]) ||
    typeof record["verifier"] !== "string" ||
    !VERIFIER.test(record["verifier"]) ||
    typeof record["returnTo"] !== "string" ||
    safeReturnPath(record["returnTo"]) !== record["returnTo"] ||
    typeof record["issuedAt"] !== "number" ||
    !Number.isSafeInteger(record["issuedAt"])
  ) {
    return null;
  }
  return {
    provider: record["provider"],
    verifier: record["verifier"],
    returnTo: record["returnTo"],
    issuedAt: record["issuedAt"],
  };
}

export function isFreshState(state: SignInState, now = Date.now()): boolean {
  return state.issuedAt <= now + CLOCK_SKEW_MS && now - state.issuedAt <= STATE_TTL_MS;
}

function encodeCanonicalJson(body: DashboardSessionRequest): Uint8Array {
  return new TextEncoder().encode(JSON.stringify({
    code: body.code,
    codeVerifier: body.codeVerifier,
    state: body.state,
  }));
}

/**
 * The anonymous sign-in call has its own server-only path. It is intentionally
 * absent from `DASHBOARD_ROUTES`: the generic passthrough requires an existing
 * browser credential, while this operation is what mints the first one.
 */
export async function openDashboardSession(
  input: DashboardSessionRequest,
  transport: AexTransport = transportFor("central", null),
): Promise<DashboardSessionCredential> {
  const body = DashboardSessionRequestSchema.parse(input);
  const route = ROUTES.dashboard_session_create;
  const request: WireRequest = {
    routeId: route.id,
    method: route.method,
    path: route.path,
    headers: new Headers({
      accept: "application/json",
      "content-type": "application/json",
      "Aex-Client": CLIENT_HEADER,
    }),
    body: encodeCanonicalJson(body),
    signal: AbortSignal.timeout(10_000),
  };
  let response;
  try {
    response = await transport.execute<unknown>(request);
  } catch {
    throw new SignInError("exchange_unavailable", "the identity service did not answer");
  }
  if (response.status !== 201) {
    const failure: SignInFailure = response.status >= 400 && response.status < 429
      ? "exchange_refused"
      : "exchange_unavailable";
    throw new SignInError(failure, `the identity service answered ${response.status}`);
  }
  const credential = DashboardSessionCredentialSchema.safeParse(response.body);
  if (!credential.success) {
    throw new SignInError("exchange_unavailable", "the identity service returned no credential");
  }
  return credential.data;
}

export type CloseResult =
  | { readonly kind: "closed" }
  | { readonly kind: "failed"; readonly status: number; readonly code: string; readonly message: string };

export async function closeDashboardSession(
  credential: string,
  transport: AexTransport = transportFor("central", null),
): Promise<CloseResult> {
  const route = ROUTES.dashboard_session_delete;
  const request: WireRequest = {
    routeId: route.id,
    method: route.method,
    path: route.path,
    headers: new Headers({
      authorization: `Bearer ${credential}`,
      accept: "application/json",
      "Aex-Client": CLIENT_HEADER,
    }),
    signal: AbortSignal.timeout(10_000),
  };
  let response;
  try {
    response = await transport.execute<unknown>(request);
  } catch {
    return {
      kind: "failed",
      status: 504,
      code: "upstream_error",
      message: "the identity service did not answer in time",
    };
  }
  if (response.status === 204 || response.status === 401) return { kind: "closed" };
  if (response.status < 400) {
    return {
      kind: "failed",
      status: 502,
      code: "upstream_error",
      message: "the identity service did not confirm the session was closed",
    };
  }
  const error = apiErrorFromResponse(
    route.id,
    response.status,
    response.body,
    response.headers,
  );
  return { kind: "failed", status: response.status, code: error.code, message: error.message };
}

export function sessionLifetimeSeconds(expiresAt: string, now = Date.now()): number {
  const expiry = Date.parse(expiresAt);
  if (!Number.isFinite(expiry)) {
    throw new SignInError("exchange_unavailable", "the session has no valid expiry");
  }
  const seconds = Math.floor((expiry - now) / 1_000);
  if (seconds <= 0) {
    throw new SignInError("exchange_unavailable", "the identity service returned an expired session");
  }
  return Math.min(seconds, MAX_SESSION_SECONDS);
}
