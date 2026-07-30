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
  "CREDENTIAL_INVALID",
  "API_ERROR",
  "NETWORK_ERROR"
] as const satisfies readonly AexErrorCode[];

type Assert<T extends true> = T;
type Same<A, B> = [A] extends [B] ? ([B] extends [A] ? true : false) : false;
type ErrorCodeIsExact = Assert<
  Same<AexErrorCode, (typeof AEX_ERROR_CODES)[number]>
>;
const errorCodeIsExact: ErrorCodeIsExact = true;

describe("public v1 error ownership", () => {
  it("keeps the package root and owner modules on one runtime identity", () => {
    expect(root.AexError).toBe(DirectAexError);
    expect(root.AexApiError).toBe(DirectAexApiError);
    expect(root.AexNetworkError).toBe(DirectAexNetworkError);
    expect(root.extractErrorCode).toBe(directExtractErrorCode);
    expect(root.HttpClient).toBe(DirectHttpClient);
  });

  it("pins the clean client exception taxonomy and complete wire messages", () => {
    expect(errorCodeIsExact).toBe(true);
    expect(Object.keys(root.AEX_API_ERROR_MESSAGES))
      .toEqual([...root.AEX_API_ERROR_CODES]);
  });

  it("walks at most five causal levels when extracting diagnostic codes", () => {
    const nested = (levels: number, code: string): Error => {
      let current: Error = Object.assign(new Error("coded"), { code });
      for (let index = 1; index < levels; index += 1) {
        current = new Error(`level-${index}`, { cause: current });
      }
      return current;
    };
    expect(root.extractErrorCode(nested(5, "ECONNREFUSED")))
      .toBe("ECONNREFUSED");
    expect(root.extractErrorCode(nested(6, "EOUTOFRANGE"))).toBeUndefined();
  });
});
