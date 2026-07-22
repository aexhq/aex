import { describe, expect, it } from "vitest";
import {
  CONTRACT_PARSE_ERROR,
  isContractParseError,
  parseApiKey,
  parseApprovalGate,
  parseAssetRefFields,
  parseBundleManifest,
  parseDurationToMs,
  parseInlineSecrets,
  parseMcpServerRef,
  parseModelName,
  parseProviderFault,
  parseProviderName,
  parseResponseFormat,
  parseRuntimeSize,
  parseRuntimeKind,
  parseSessionLimits,
  parseSessionRequestConfig,
  parseSessionTimeout,
  parseSessionWebhook,
  parseSkillBundleEntry,
  parseSkillBundleManifest,
  parseSubmission,
  tryParseApiKey,
  tryParseBundleManifest,
  validateSkillBundleEntry,
  validateSkillBundleManifest,
  type ContractParseError
} from "../src/index.js";
import {
  parsePostHook,
  parseRetryAfterMs,
  parseRuntimeSecurityProfile,
  parseSessionMachine,
  parseSessionSubmissionRequest,
  tryParseRetryAfterMs
} from "../src/internal.js";

describe("contract validation conventions", () => {
  it("keeps nullable recognizer compatibility wrappers identical", () => {
    for (const token of ["opaque", "aex_dev_euw1_w_s_t", ""] as const) {
      expect(parseApiKey(token)).toEqual(tryParseApiKey(token));
    }
    for (const bytes of [undefined, null, new Uint8Array(), new TextEncoder().encode("{}")]) {
      expect(parseBundleManifest(bytes)).toEqual(tryParseBundleManifest(bytes));
    }
    for (const header of [undefined, null, "", "2", "invalid"] as const) {
      expect(parseRetryAfterMs(header, 0)).toBe(tryParseRetryAfterMs(header, 0));
    }
  });

  it("keeps strict skill compatibility wrappers identical", () => {
    const entry = { path: "SKILL.md", size: 1, mode: 0o644 } as const;
    expect(validateSkillBundleEntry(entry)).toEqual(parseSkillBundleEntry(entry));
    expect(validateSkillBundleManifest([entry])).toEqual(parseSkillBundleManifest([entry]));
  });

  it("brands the original strict-parser error with immutable non-enumerable metadata", () => {
    let thrown: unknown;
    try {
      parseRuntimeKind("invalid");
    } catch (error) {
      thrown = error;
    }
    expect(thrown).toBeInstanceOf(Error);
    expect(isContractParseError(thrown)).toBe(true);
    const parsed = thrown as ContractParseError;
    expect(parsed.code).toBe(CONTRACT_PARSE_ERROR);
    expect(parsed.parser).toBe("parseRuntimeKind");
    expect(Object.keys(parsed)).toEqual([]);
    expect(Object.getOwnPropertyDescriptor(parsed, "code")).toMatchObject({
      enumerable: false,
      writable: false
    });
    expect(Object.getOwnPropertyDescriptor(parsed, "parser")).toMatchObject({
      enumerable: false,
      writable: false
    });
  });

  it.each([
    ["parseModelName", () => parseModelName("invalid")],
    ["parseProviderFault", () => parseProviderFault(null)],
    ["parseRuntimeKind", () => parseRuntimeKind("invalid")],
    ["parseRuntimeSize", () => parseRuntimeSize("invalid")],
    ["parseDurationToMs", () => parseDurationToMs("invalid")],
    ["parseSessionTimeout", () => parseSessionTimeout(10)],
    ["parsePostHook", () => parsePostHook(10)],
    ["parseRuntimeSecurityProfile", () => parseRuntimeSecurityProfile("invalid")],
    ["parseAssetRefFields", () => parseAssetRefFields({}, "asset")],
    ["parseMcpServerRef", () => parseMcpServerRef(null, "mcp")],
    ["parseSessionRequestConfig", () => parseSessionRequestConfig(null)],
    ["parseSkillBundleEntry", () => parseSkillBundleEntry({ path: "../x", size: 1 })],
    ["parseSkillBundleManifest", () => parseSkillBundleManifest([])],
    ["parseInlineSecrets", () => parseInlineSecrets({ unexpected: true })],
    ["parseSessionSubmissionRequest", () => parseSessionSubmissionRequest({ unexpected: true })],
    ["parseSessionWebhook", () => parseSessionWebhook(10)],
    ["parseSessionLimits", () => parseSessionLimits(10)],
    ["parseSessionMachine", () => parseSessionMachine(10)],
    ["parseProviderName", () => parseProviderName("invalid")],
    ["parseSubmission", () => parseSubmission(null)],
    ["parseResponseFormat", () => parseResponseFormat(10)],
    ["parseApprovalGate", () => parseApprovalGate(10)]
  ])("brands direct %s failures", (parser, invoke) => {
    let thrown: unknown;
    try {
      invoke();
    } catch (error) {
      thrown = error;
    }
    expect(isContractParseError(thrown)).toBe(true);
    expect((thrown as ContractParseError).parser).toBe(parser);
  });

  it("preserves the nearest marked parser through nested strict parsing", () => {
    let thrown: unknown;
    try {
      parseSessionTimeout("invalid");
    } catch (error) {
      thrown = error;
    }
    expect(isContractParseError(thrown)).toBe(true);
    expect((thrown as ContractParseError).parser).toBe("parseDurationToMs");
  });
});
