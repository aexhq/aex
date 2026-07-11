import { describe, expect, it } from "vitest";
// @ts-expect-error JS release helper is validated directly.
import { assertMonotonicPromotion } from "../cicd/assert-monotonic-promotion.mjs";

const mainSha = "0123456789abcdef0123456789abcdef01234567";

describe("monotonic promotion guard", () => {
  it("accepts a tested main ancestor when its version advances latest", () => {
    expect(assertMonotonicPromotion({
      candidateVersion: "0.14.0-canary.82.g0123456789ab",
      candidateSha: mainSha,
      currentMainSha: "f".repeat(40),
      candidateIsMainAncestor: true,
      currentLatestVersion: "0.13.9"
    })).toMatchObject({
      candidateVersion: "0.14.0-canary.82.g0123456789ab",
      candidateSha: mainSha
    });
  });

  it("rejects a candidate that is not in current main history", () => {
    expect(() => assertMonotonicPromotion({
      candidateVersion: "0.14.0-canary.82.g0123456789ab",
      candidateSha: mainSha,
      currentMainSha: "f".repeat(40),
      candidateIsMainAncestor: false,
      currentLatestVersion: "0.13.9"
    })).toThrow(/not an ancestor/);
  });

  it("rejects promotion that would move latest backwards", () => {
    expect(() => assertMonotonicPromotion({
      candidateVersion: "0.13.9-canary.82.g0123456789ab",
      candidateSha: mainSha,
      currentMainSha: mainSha,
      candidateIsMainAncestor: true,
      currentLatestVersion: "0.13.9"
    })).toThrow(/older than current latest/);
  });

  it("fails closed on malformed registry or source data", () => {
    expect(() => assertMonotonicPromotion({
      candidateVersion: "next",
      candidateSha: mainSha,
      currentMainSha: mainSha,
      candidateIsMainAncestor: true,
      currentLatestVersion: "0.13.9"
    })).toThrow(/invalid candidate version/);
    expect(() => assertMonotonicPromotion({
      candidateVersion: "0.14.0",
      candidateSha: mainSha,
      currentMainSha: mainSha,
      candidateIsMainAncestor: true,
      currentLatestVersion: ""
    })).toThrow(/invalid current latest version/);
  });
});
