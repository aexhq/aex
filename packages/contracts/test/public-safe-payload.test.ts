import { describe, expect, it } from "vitest";
import {
  isPlatformContentDigest,
  scanCustodyPayloadForSensitiveValues,
  scanSessionRetentionPayloadForSensitiveValues,
  scanSideEffectAuditPayloadForSensitiveValues
} from "../src/internal.js";

const scanners = {
  custody: scanCustodyPayloadForSensitiveValues,
  retention: scanSessionRetentionPayloadForSensitiveValues,
  audit: scanSideEffectAuditPayloadForSensitiveValues
} as const;

const sharedSecretCorpus = [
  ["bearer_token", "Bearer synthetic-runner-token-1234567890"],
  ["provider_key", "sk-synthetic1a2b3c4d5e6f7g8h9"],
  ["provider_key", "sk-ant-synthetic-1234567890"],
  ["provider_key", "xoxb-synthetic-1234567890"],
  ["provider_key", "AIzaSynthetic1234567890"],
  ["signed_url", "https://objects.example.test/a?X-Amz-Signature=synthetic"],
  ["object_store_key", "sessions/session-synthetic/files/result.txt"],
  ["vault_id", "vault_synthetic1234567890"],
  ["private_resource_handle", "machine_synthetic1234567890"],
  ["high_entropy_token", "Zx9Kq2Lp7Vn4Rt6Wy8Ub3Mc5Ad1Ef0Gh2Ij4Kl6mnopQRstuv99"]
] as const;

const hex = "0123456789abcdef";
const digest64 = hex.repeat(4);

describe("canonical public-safe payload corpus", () => {
  it("keeps opaque underscore-bearing audit identifiers covered", () => {
    expect(
      scanSideEffectAuditPayloadForSensitiveValues({
        target: {
          id: "Zx9_0a98965d822f209f28604ce77e1de5a91e95b7122513a9594d12fee9a5a1"
        }
      })
    ).toEqual([
      { path: "$.target.id", reason: "high_entropy_token", valueLength: 64 }
    ]);
  });

  it.each(sharedSecretCorpus)("flags %s in every consumer", (reason, value) => {
    for (const [consumer, scan] of Object.entries(scanners)) {
      expect(scan({ note: value }).map((finding) => finding.reason), consumer).toContain(reason);
    }
  });

  it("applies the lowercase SHA-256 carve-out only in approved contexts", () => {
    expect(scanCustodyPayloadForSensitiveValues({ note: digest64 })).toEqual([]);
    expect(scanSessionRetentionPayloadForSensitiveValues({ note: digest64 }).map((f) => f.reason))
      .toContain("high_entropy_token");
    expect(scanSideEffectAuditPayloadForSensitiveValues({ target: { id: digest64 } })).toEqual([]);
    expect(scanSideEffectAuditPayloadForSensitiveValues({ note: digest64 }).map((f) => f.reason))
      .toContain("high_entropy_token");
  });

  it.each([
    ["lowercase 32-hex", hex.repeat(2)],
    ["lowercase 40-hex", `${hex.repeat(2)}01234567`],
    ["lowercase 56-hex", `${hex.repeat(3)}01234567`],
    ["uppercase 64-hex", digest64.toUpperCase()],
    ["sha256-prefixed", `sha256:${digest64}`]
  ])("does not treat %s as a platform content digest", (_name, value) => {
    expect(isPlatformContentDigest(value)).toBe(false);
  });

  it("recognizes only a bare lowercase 64-hex SHA-256 digest", () => {
    expect(isPlatformContentDigest(digest64)).toBe(true);
    expect(isPlatformContentDigest(`${digest64}0`)).toBe(false);
    expect(isPlatformContentDigest(digest64.slice(1))).toBe(false);
    expect(isPlatformContentDigest(`${digest64.slice(0, -1)}g`)).toBe(false);
  });

  it("is deterministic and does not throw on cyclic or accessor-bearing input", () => {
    const cyclic: Record<string, unknown> = { note: "safe" };
    cyclic.self = cyclic;
    const accessor = Object.create(null) as Record<string, unknown>;
    Object.defineProperty(accessor, "note", {
      enumerable: true,
      get() {
        throw new Error("must not be invoked");
      }
    });

    for (const scan of Object.values(scanners)) {
      const first = scan(cyclic);
      expect(first).toEqual(scan(cyclic));
      expect(first.map((finding) => finding.reason)).toContain("uninspectable_value");
      expect(() => scan(accessor)).not.toThrow();
      expect(scan(accessor).map((finding) => finding.reason)).toContain("uninspectable_value");
    }
  });
});
