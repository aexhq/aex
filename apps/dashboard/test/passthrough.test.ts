import { describe, expect, test } from "bun:test";

import { MAX_BODY_BYTES, readCookie, resolvePassthrough } from "../src/server/passthrough";

const SESSION = "__Host-aex_session=aex_ds_fixture";
const CSRF = "__Host-aex_csrf=token";

function request(
  url: string,
  init: { method?: string; cookie?: string; headers?: Record<string, string> } = {},
): Request {
  const headers = new Headers(init.headers ?? {});
  if (init.cookie) headers.set("cookie", init.cookie);
  return new Request(`https://dash.example${url}`, { method: init.method ?? "GET", headers });
}

function refusal(result: ReturnType<typeof resolvePassthrough>) {
  return result.ok ? null : result.refusal;
}

describe("route admission", () => {
  test("an unknown operation is refused before the credential is read", () => {
    const result = resolvePassthrough("regional", "session_purge", request("/api/v1/regional/session_purge"), null);
    expect(refusal(result)).toEqual({ status: 404, code: "not_found", message: "unknown operation" });
  });

  test("an allowlisted operation on the wrong plane is refused", () => {
    const result = resolvePassthrough(
      "central",
      "sessions_list",
      request("/api/v1/central/sessions_list", { cookie: SESSION }),
      null,
    );
    expect(refusal(result)?.status).toBe(404);
  });

  test("the wrong method is refused before the credential is read", () => {
    const result = resolvePassthrough(
      "central",
      "api_keys_list",
      request("/api/v1/central/api_keys_list", { method: "DELETE" }),
      null,
    );
    expect(refusal(result)?.status).toBe(405);
  });

  test("a request with no browser session is refused with no upstream call", () => {
    const result = resolvePassthrough(
      "central",
      "api_keys_list",
      request("/api/v1/central/api_keys_list?workspaceId=wsp_1"),
      null,
    );
    expect(refusal(result)).toEqual({
      status: 401,
      code: "unauthenticated",
      message: "browser session required",
    });
  });
});

describe("cross-site protection", () => {
  test("a mutation without a matching CSRF token is refused", () => {
    const result = resolvePassthrough(
      "regional",
      "secret_put",
      request("/api/v1/regional/secret_put?region=euw1&name=token", {
        method: "PUT",
        cookie: `${SESSION}; ${CSRF}`,
        headers: { "x-aex-csrf": "other", "sec-fetch-site": "same-origin" },
      }),
      new TextEncoder().encode(JSON.stringify({ value: "v" })),
    );
    expect(refusal(result)?.status).toBe(403);
  });

  test("a cross-site mutation is refused even with a matching token", () => {
    const result = resolvePassthrough(
      "regional",
      "secret_put",
      request("/api/v1/regional/secret_put?region=euw1&name=token", {
        method: "PUT",
        cookie: `${SESSION}; ${CSRF}`,
        headers: { "x-aex-csrf": "token", "sec-fetch-site": "cross-site" },
      }),
      new TextEncoder().encode(JSON.stringify({ value: "v" })),
    );
    expect(refusal(result)?.status).toBe(403);
  });
});

describe("parameter admission", () => {
  test("a regional operation requires a region from the closed set", () => {
    const missing = resolvePassthrough(
      "regional",
      "sessions_list",
      request("/api/v1/regional/sessions_list", { cookie: SESSION }),
      null,
    );
    expect(refusal(missing)?.status).toBe(400);

    const bogus = resolvePassthrough(
      "regional",
      "sessions_list",
      request("/api/v1/regional/sessions_list?region=elsewhere", { cookie: SESSION }),
      null,
    );
    expect(refusal(bogus)?.status).toBe(400);
  });

  test("a path parameter is bound into the path and never concatenated raw", () => {
    const result = resolvePassthrough(
      "regional",
      "session_get",
      request("/api/v1/regional/session_get?region=euw1&sessionId=ses_01", { cookie: SESSION }),
      null,
    );
    expect(result.ok).toBe(true);
    expect(result.ok && result.request.path).toBe("/api/sessions/ses_01");
  });

  test("a traversal attempt in a path parameter is refused", () => {
    const result = resolvePassthrough(
      "regional",
      "session_get",
      request("/api/v1/regional/session_get?region=euw1&sessionId=..%2F..%2Fadmin", { cookie: SESSION }),
      null,
    );
    expect(refusal(result)?.status).toBe(400);
  });

  test("only the query parameters the contract declares are forwarded", () => {
    const allowed = resolvePassthrough(
      "central",
      "api_keys_list",
      request("/api/v1/central/api_keys_list?workspaceId=wsp_1&limit=10", { cookie: SESSION }),
      null,
    );
    // Forwarding follows the contract's declared parameter order, so the upstream
    // path is byte-identical for the same inputs regardless of how the browser
    // ordered them.
    expect(allowed.ok && allowed.request.path).toBe("/api/api-keys?limit=10&workspaceId=wsp_1");

    const extra = resolvePassthrough(
      "central",
      "api_keys_list",
      request("/api/v1/central/api_keys_list?workspaceId=wsp_1&organizationId=org_1", { cookie: SESSION }),
      null,
    );
    expect(refusal(extra)?.status).toBe(400);
  });
});

describe("mutation admission", () => {
  const mutationHeaders = { "x-aex-csrf": "token", "sec-fetch-site": "same-origin" };

  test("an idempotency-key operation without the header is refused", () => {
    const result = resolvePassthrough(
      "regional",
      "secret_put",
      request("/api/v1/regional/secret_put?region=euw1&name=token", {
        method: "PUT",
        cookie: `${SESSION}; ${CSRF}`,
        headers: mutationHeaders,
      }),
      new TextEncoder().encode(JSON.stringify({ value: "v" })),
    );
    expect(refusal(result)?.status).toBe(400);
  });

  test("a well-formed mutation carries the credential, the key and a canonical body", () => {
    const result = resolvePassthrough(
      "regional",
      "secret_put",
      request("/api/v1/regional/secret_put?region=euw1&name=token", {
        method: "PUT",
        cookie: `${SESSION}; ${CSRF}`,
        headers: { ...mutationHeaders, "idempotency-key": "idk_1" },
      }),
      new TextEncoder().encode(JSON.stringify({ value: "v" })),
    );
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.request.path).toBe("/api/secrets/token");
    expect(result.request.headers.get("authorization")).toBe("Bearer aex_ds_fixture");
    expect(result.request.headers.get("Idempotency-Key")).toBe("idk_1");
    expect(new TextDecoder().decode(result.request.body)).toBe('{"value":"v"}');
  });

  test("an oversized body is refused before it is parsed", () => {
    const result = resolvePassthrough(
      "regional",
      "secret_put",
      request("/api/v1/regional/secret_put?region=euw1&name=token", {
        method: "PUT",
        cookie: `${SESSION}; ${CSRF}`,
        headers: { ...mutationHeaders, "idempotency-key": "idk_1" },
      }),
      new Uint8Array(MAX_BODY_BYTES + 1),
    );
    expect(refusal(result)?.status).toBe(413);
  });

  test("a body that is not JSON is refused", () => {
    const result = resolvePassthrough(
      "regional",
      "secret_put",
      request("/api/v1/regional/secret_put?region=euw1&name=token", {
        method: "PUT",
        cookie: `${SESSION}; ${CSRF}`,
        headers: { ...mutationHeaders, "idempotency-key": "idk_1" },
      }),
      new TextEncoder().encode("value=v"),
    );
    expect(refusal(result)?.status).toBe(400);
  });
});

test("cookie reading returns exactly the named cookie", () => {
  expect(readCookie("a=1; __Host-aex_session=aex_ds_x; b=2", "__Host-aex_session")).toBe("aex_ds_x");
  expect(readCookie("a=1", "__Host-aex_session")).toBeNull();
  expect(readCookie(null, "__Host-aex_session")).toBeNull();
});

test("a malformed cookie value is absent rather than an exception", () => {
  expect(readCookie("__Host-aex_signin=%", "__Host-aex_signin")).toBeNull();
  expect(readCookie("__Host-aex_session=%E0%A4%A", "__Host-aex_session")).toBeNull();
});
