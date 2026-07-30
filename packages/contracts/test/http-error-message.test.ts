import { describe, expect, it } from "bun:test";
import { AexApiError, HttpClient } from "../src/index.js";

describe("v1 nested API error envelope", () => {
  it("surfaces the nested message, code, request id, and details", async () => {
    const body = {
      error: {
        code: "account_paused",
        message: "Top up before starting new work.",
        requestId: "req_body",
        retryable: false,
        details: { minimumRestoreCents: "500" }
      }
    };
    const client = new HttpClient({
      baseUrl: "https://api.example.test",
      apiKey: "test",
      fetch: async () => Response.json(body, { status: 402 })
    });
    const error = await client.request("/account").catch((caught) => caught);
    expect(error).toBeInstanceOf(AexApiError);
    expect((error as AexApiError).message)
      .toBe("Top up before starting new work.");
    expect((error as AexApiError).apiCode).toBe("account_paused");
    expect((error as AexApiError).requestId).toBe("req_body");
    expect((error as AexApiError).body).toEqual(body);
  });

  it("uses a response request-id header only when the envelope omitted it", async () => {
    const client = new HttpClient({
      baseUrl: "https://api.example.test",
      apiKey: "test",
      fetch: async () => Response.json({
        error: {
          code: "internal_error",
          message: "failed",
          retryable: false
        }
      }, {
        status: 500,
        headers: { "x-request-id": "req_header" }
      })
    });
    const error = await client.request("/sessions").catch((caught) => caught);
    expect((error as AexApiError).requestId).toBe("req_header");
  });
});
