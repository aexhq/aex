import { describe, expect, test } from "bun:test";
import type { AexTransport, WireRequest, WireResponse, WireStreamResponse } from "@aexhq/sdk";
import { DashboardSessionRequestSchema, newId } from "@aexhq/wire";

import {
  STATE_TTL_MS,
  authorizationUrl,
  callbackUrl,
  challengeOf,
  closeDashboardSession,
  configuredProviders,
  decodeSignInState,
  encodeSignInState,
  isFreshState,
  isProviderId,
  openDashboardSession,
  signInConfig,
} from "../src/server/signin";
import { csrfCookie, dashboardSessionCookie, signInStateCookie } from "../src/server/session";
import { centralBaseUrl, regionalBaseUrl } from "../src/server/upstream";

const VERIFIER = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
const STATE = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

function fakeTransport(response: WireResponse<unknown>): {
  readonly transport: AexTransport;
  readonly requests: WireRequest[];
} {
  const requests: WireRequest[] = [];
  return {
    requests,
    transport: {
      execute<T>(request: WireRequest): Promise<WireResponse<T>> {
        requests.push(request);
        return Promise.resolve(response as WireResponse<T>);
      },
      stream<T>(): Promise<WireStreamResponse<T>> {
        throw new Error("this identity test never streams");
      },
    },
  };
}

describe("provider configuration", () => {
  test("the dashboard accepts the complete public provider configuration", () => {
    const environment = {
      AEX_DASHBOARD_ORIGIN: "https://dev.aex.dev",
      AEX_CENTRAL_URL: "https://dev-api.aex.dev",
      AEX_OAUTH_GOOGLE_CLIENT_ID: "google-client",
    };
    const config = signInConfig(environment);
    expect(config).toEqual({
      kind: "ready",
      origin: "https://dev.aex.dev",
      central: "https://dev-api.aex.dev",
      clientIds: { google: "google-client" },
    });
    expect(configuredProviders(environment)).toEqual(["google"]);
    expect(isProviderId("github")).toBe(false);
    expect(DashboardSessionRequestSchema.safeParse({
      code: "one-use-code",
      state: STATE,
      codeVerifier: VERIFIER,
    }).success).toBe(true);
    expect(JSON.stringify(config).toLowerCase()).not.toContain("secret");
  });

  test("partial sign-in configuration fails closed", () => {
    expect(() => signInConfig({
      AEX_DASHBOARD_ORIGIN: "https://dev.aex.dev",
      AEX_OAUTH_GOOGLE_CLIENT_ID: "google-client",
    })).toThrow("AEX_CENTRAL_URL");
    expect(() => signInConfig({
      AEX_CENTRAL_URL: "https://dev-api.aex.dev",
      AEX_OAUTH_GOOGLE_CLIENT_ID: "google-client",
    })).toThrow("AEX_DASHBOARD_ORIGIN");
    expect(() => signInConfig({
      AEX_DASHBOARD_ORIGIN: "https://dev.aex.dev",
      AEX_CENTRAL_URL: "https://dev-api.aex.dev",
    })).toThrow("AEX_OAUTH_GOOGLE_CLIENT_ID");
  });

  test("the central API origin is explicit and regional traffic stays in its plane", () => {
    expect(centralBaseUrl({ AEX_CENTRAL_URL: "https://dev-api.aex.dev" }))
      .toBe("https://dev-api.aex.dev");
    expect(regionalBaseUrl("euw1", { AEX_CENTRAL_URL: "https://dev-api.aex.dev" }))
      .toBe("https://eu-west-1.dev-api.aex.dev");
    expect(regionalBaseUrl("euw1", { AEX_CENTRAL_URL: "https://api.aex.dev" }))
      .toBe("https://eu-west-1.api.aex.dev");
    expect(() => centralBaseUrl({})).toThrow("AEX_CENTRAL_URL");
    expect(() => centralBaseUrl({ AEX_CENTRAL_URL: "" })).toThrow("AEX_CENTRAL_URL");
  });

  test("a provider endpoint pins its exact authority, callback and S256 binding", async () => {
    expect(await challengeOf(VERIFIER)).toBe(STATE);
    const redirectUri = callbackUrl("https://dash.aex.dev");
    expect(redirectUri).toBe("https://dash.aex.dev/api/auth/callback");

    const parsed = new URL(authorizationUrl("google", "google-client", redirectUri, STATE));
    expect(parsed.protocol).toBe("https:");
    expect(parsed.host).toBe("accounts.google.com");
    expect(parsed.pathname).toBe("/o/oauth2/v2/auth");
    expect(parsed.searchParams.get("client_id")).toBe("google-client");
    expect(parsed.searchParams.get("redirect_uri")).toBe(redirectUri);
    expect(parsed.searchParams.get("state")).toBe(STATE);
    expect(parsed.searchParams.get("code_challenge")).toBe(STATE);
    expect(parsed.searchParams.get("code_challenge_method")).toBe("S256");
    expect(parsed.searchParams.get("nonce")).toBe(STATE);
  });
});

describe("the host-only sign-in binding", () => {
  test("the cookie carries provider, verifier, return path and issue time", () => {
    const binding = {
      provider: "google",
      verifier: VERIFIER,
      returnTo: "/w/wsp_example/sessions",
      issuedAt: 1_754_000_000_000,
    } as const;
    expect(decodeSignInState(encodeSignInState(binding))).toEqual(binding);
  });

  test("a malformed, unsafe or stale binding is refused", () => {
    const issuedAt = 1_754_000_000_000;
    const binding = {
      provider: "google",
      verifier: VERIFIER,
      returnTo: "/w/alpha/sessions",
      issuedAt,
    } as const;
    expect(isFreshState(binding, issuedAt)).toBe(true);
    expect(isFreshState(binding, issuedAt + STATE_TTL_MS)).toBe(true);
    expect(isFreshState(binding, issuedAt + STATE_TTL_MS + 1)).toBe(false);
    expect(decodeSignInState("not-base64url")).toBeNull();

    const unsafe = Buffer.from(JSON.stringify({ ...binding, returnTo: "//evil.example" }))
      .toString("base64url");
    expect(decodeSignInState(unsafe)).toBeNull();
  });
});

describe("the dedicated identity transport", () => {
  test("session creation uses the generated route without a bearer credential", async () => {
    const { transport, requests } = fakeTransport({
      status: 201,
      headers: new Headers(),
      body: {
        session: "aex_ds_fixture_1234",
        userId: newId("user"),
        expiresAt: "2026-08-11T00:00:00.000Z",
      },
    });
    const credential = await openDashboardSession(
      { code: "one-use-code", state: STATE, codeVerifier: VERIFIER },
      transport,
    );
    expect(credential.session).toBe("aex_ds_fixture_1234");
    expect(requests).toHaveLength(1);
    expect(requests[0]).toMatchObject({
      routeId: "dashboard_session_create",
      method: "POST",
      path: "/api/auth/sessions",
    });
    expect(requests[0]!.headers.get("authorization")).toBeNull();
    expect(JSON.parse(new TextDecoder().decode(requests[0]!.body))).toEqual({
      code: "one-use-code",
      codeVerifier: VERIFIER,
      state: STATE,
    });
  });

  test("central logout presents the session and accepts an already-closed credential", async () => {
    for (const status of [204, 401]) {
      const { transport, requests } = fakeTransport({
        status,
        headers: new Headers(),
        body: undefined,
      });
      expect(await closeDashboardSession("aex_ds_fixture", transport)).toEqual({ kind: "closed" });
      expect(requests[0]).toMatchObject({
        routeId: "dashboard_session_delete",
        method: "DELETE",
        path: "/api/auth/sessions/current",
      });
      expect(requests[0]!.headers.get("authorization")).toBe("Bearer aex_ds_fixture");
    }
  });
});

describe("browser credentials", () => {
  test("session, CSRF and sign-in cookies all use the host-only envelope", () => {
    const cookies = [
      dashboardSessionCookie("aex_ds_fixture", 600),
      csrfCookie(VERIFIER, 600),
      signInStateCookie(
        encodeSignInState({ provider: "google", verifier: VERIFIER, returnTo: "/", issuedAt: 1 }),
        600,
      ),
    ];
    for (const cookie of cookies) {
      expect(cookie).toContain("__Host-");
      expect(cookie).toContain("Secure");
      expect(cookie).toContain("Path=/");
      expect(cookie).not.toContain("Domain=");
    }
    expect(cookies[0]).toContain("HttpOnly");
    expect(cookies[2]).toContain("HttpOnly");
  });
});
