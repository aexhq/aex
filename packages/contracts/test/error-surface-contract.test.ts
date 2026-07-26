import { describe, expect, it } from "bun:test";
import * as root from "../src/index.js";
import {
  AexApiError as DirectAexApiError,
  AexError as DirectAexError,
  AexNetworkError as DirectAexNetworkError,
  extractErrorCode as directExtractErrorCode,
  type AexErrorCode
} from "../src/sdk-errors.js";
import { HttpClient as DirectHttpClient } from "../src/http.js";

const AEX_ERROR_CODES = [
  "SESSION_CONFIG_INVALID",
  "CREDENTIAL_INVALID",
  "PROVIDER_ERROR",
  "SESSION_STATE_ERROR",
  "CLEANUP_ERROR",
  "RUNTIME_UNSUPPORTED",
  "API_ERROR",
  "NETWORK_ERROR"
] as const satisfies readonly AexErrorCode[];

type Assert<T extends true> = T;
type Same<A, B> = [A] extends [B] ? ([B] extends [A] ? true : false) : false;
type ErrorCodeIsExact = Assert<Same<AexErrorCode, (typeof AEX_ERROR_CODES)[number]>>;
const errorCodeIsExact: ErrorCodeIsExact = true;

// @ts-expect-error TEMPLATE_INVALID is a retired private-platform drift value.
const retiredTemplateCode: AexErrorCode = "TEMPLATE_INVALID";
void retiredTemplateCode;

const AEX_API_ERROR_CODES = [
  "unauthorized",
  "forbidden",
  "insufficient_scope",
  "token_invalid",
  "token_revoked",
  "token_expired",
  "malformed_token",
  "not_found",
  "idempotency_conflict",
  "session_busy",
  "checkpoint_not_available",
  "session_not_terminal",
  "session_terminal",
  "event_archive_too_large",
  "event_archive_deadline_exceeded",
  "unknown_workspace",
  "workspace_inactive",
  "workspace_concurrency_exceeded",
  "workspace_submit_rate_exceeded",
  "workspace_spend_cap_exceeded",
  "workspace_cap_exceeded",
  "insufficient_credits",
  "account_blocked",
  "subscription_past_due",
  "quota_exhausted",
  "depth_exceeded",
  "rate_limited",
  "out_of_memory",
  "disk_full",
  "content_deleted",
  "upstream_error",
  "internal_error"
] as const;

describe("public error surface ownership", () => {
  it("keeps the package root and owner modules on one runtime identity", () => {
    expect(root.AexError).toBe(DirectAexError);
    expect(root.AexApiError).toBe(DirectAexApiError);
    expect(root.AexNetworkError).toBe(DirectAexNetworkError);
    expect(root.extractErrorCode).toBe(directExtractErrorCode);
    expect(root.HttpClient).toBe(DirectHttpClient);
  });

  it("pins the exact client exception and serialized wire taxonomies", () => {
    expect(errorCodeIsExact).toBe(true);
    expect(root.AEX_API_ERROR_CODES).toEqual(AEX_API_ERROR_CODES);
    expect(Object.keys(root.AEX_API_ERROR_MESSAGES)).toEqual([...AEX_API_ERROR_CODES]);
    for (const code of AEX_API_ERROR_CODES) {
      expect(root.AEX_API_ERROR_MESSAGES[code], code).toBeTypeOf("string");
      expect(root.AEX_API_ERROR_MESSAGES[code].length, code).toBeGreaterThan(0);
    }
  });

  it("keeps typed wire-to-exception dispatch at the public owner", () => {
    expect(root.apiErrorFromResponse({ status: 401, body: { error: "token_invalid" } }))
      .toBeInstanceOf(root.AexAuthError);
    expect(root.apiErrorFromResponse({ status: 409, body: { error: "idempotency_conflict" } }))
      .toBeInstanceOf(root.AexIdempotencyConflictError);
    expect(root.apiErrorFromResponse({ status: 404, body: { error: "not_found" } }))
      .toBeInstanceOf(root.AexNotFoundError);
    expect(root.apiErrorFromResponse({ status: 429, body: { error: "rate_limited" } }))
      .toBeInstanceOf(root.AexRateLimitError);
  });

  it("walks at most five causal levels when extracting diagnostic codes", () => {
    const nested = (levels: number, code: string): Error => {
      let current: Error = Object.assign(new Error("coded"), { code });
      for (let index = 1; index < levels; index += 1) {
        current = new Error(`level-${index}`, { cause: current });
      }
      return current;
    };

    expect(root.extractErrorCode(nested(1, "23505"))).toBe("23505");
    expect(root.extractErrorCode(nested(5, "ECONNREFUSED"))).toBe("ECONNREFUSED");
    expect(root.extractErrorCode(nested(6, "EOUTOFRANGE"))).toBeUndefined();
  });
});
