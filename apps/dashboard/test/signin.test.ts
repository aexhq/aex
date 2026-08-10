import { describe, expect, test } from "bun:test";
import type { AexTransport, WireRequest, WireResponse } from "@aexhq/sdk";
import { newId } from "@aexhq/wire";

import {
  STATE_TTL_MS,
  authorizationUrl,
  callbackUrl,
  challengeOf,
  closeDashboardSession,
  decodeSignInState,
  encodeSignInState,
  isFreshState,
  openDashboardSession,
  signInConfig,
  type ProviderId,
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
    },
  };
}

describe("provider configuration", () => {
  test("the dashboard accepts public client ids and has no provider-secret setting", () => {
    const config = signInConfig({
      AEX_DASHBOARD_ORIGIN: "https://dash.aex.dev",
      AEX_OAUTH_GITHUB_CLIENT_ID: "github-client",
      AEX_OAUTH_GOOGLE_CLIENT_ID: "google-client",
    });
    expect(config).toEqual({
      kind: "ready",
      origin: "https://dash.aex.dev",
      clientIds: { github: "github-client", google: "google-client" },
    });
    expect(JSON.stringify(config).toLowerCase()).not.toContain("secret");
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

    const cases: readonly (readonly [ProviderId, string, string, string])[] = [
      ["github", "github-client", "github.com", "/login/oauth/authorize"],
      ["google", "google-client", "accounts.google.com", "/o/oauth2/v2/auth"],
    ];
    for (const [provider, clientId, host, path] of cases) {
      const parsed = new URL(
        authorizationUrl(provider, clientId, redirectUri, STATE),
      );
      expect(parsed.protocol).toBe("https:");
      expect(parsed.host).toBe(host);
      expect(parsed.pathname).toBe(path);
      expect(parsed.searchParams.get("client_id")).toBe(clientId);
      expect(parsed.searchParams.get("redirect_uri")).toBe(redirectUri);
      expect(parsed.searchParams.get("state")).toBe(STATE);
      expect(parsed.searchParams.get("code_challenge")).toBe(STATE);
      expect(parsed.searchParams.get("code_challenge_method")).toBe("S256");
    }
  });
});

describe("the host-only sign-in binding", () => {
  test("the cookie carries provider, verifier, return path and issue time", () => {
    const binding = {
      provider: "github",
      verifier: VERIFIER,
      returnTo: "/device?user_code=BCDFG-HJKLM",
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
      { provider: "github", code: "one-use-code", state: STATE, codeVerifier: VERIFIER },
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
      provider: "github",
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
        encodeSignInState({ provider: "github", verifier: VERIFIER, returnTo: "/", issuedAt: 1 }),
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
