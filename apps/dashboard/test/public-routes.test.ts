import { describe, expect, test } from "bun:test";
import { NextRequest } from "next/server";

import proxy from "../proxy";

describe("public web routes", () => {
  test.each(["/", "/docs", "/docs/", "/og.png"])("serves %s without a dashboard session", (path) => {
    const response = proxy(new NextRequest(`https://aex.dev${path}`));

    expect(response.status).toBe(200);
    expect(response.headers.get("x-middleware-next")).toBe("1");
  });

  test("keeps the authenticated application behind sign-in", () => {
    const response = proxy(new NextRequest("https://aex.dev/app"));

    expect(response.status).toBe(307);
    expect(response.headers.get("location")).toBe("https://aex.dev/signin?next=%2Fapp");
  });

  test("preserves a private destination for an existing session", () => {
    const request = new NextRequest("https://aex.dev/w/acme/sessions", {
      headers: { cookie: "__Host-aex_session=aex_ds_fixture" },
    });
    const response = proxy(request);

    expect(response.status).toBe(200);
    expect(response.headers.get("x-middleware-request-x-aex-return-to")).toBe("/w/acme/sessions");
  });
});
