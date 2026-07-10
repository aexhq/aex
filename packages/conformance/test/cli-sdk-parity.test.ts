import { describe, expect, it } from "vitest";
import { Aex } from "../../sdk/dist/index.js";
import type { SessionFiles, SessionStartOptions } from "../../sdk/dist/index.js";
import { CLI_VERB_NAMES, FILES_SUBVERBS, START_FLAGS as SESSION_FLAGS, findVerbSpec } from "../../cli/dist/index.js";
import {
  CLI_PARITY_BARE_LIST,
  CLI_PARITY_NOT_SURFACED,
  CLI_PARITY_PROVIDER_KEY_FLAG,
  CLI_SDK_PARITY_MANIFEST
} from "../src/index.js";

/**
 * COMPILE-TIME class-killer: these typed copies of the manifest maps fail to
 * BUILD the moment a `SessionStartOptions` / `SessionFiles` key is added,
 * removed, or renamed without a corresponding manifest entry — so a new SDK
 * capability can never silently break CLI parity. The runtime `toEqual`
 * below pins the (loosely-typed) published manifest to these typed copies so
 * the two can't drift.
 */
const SESSION_OPTION_COVERAGE = {
  provider: "--provider",
  model: "--model",
  system: "--system",
  tools: "--tool",
  skills: "--skill",
  agentsMd: "--agents-md",
  files: "--file",
  mcpServers: "--mcp",
  metadata: "--metadata",
  idempotencyKey: "--idempotency-key",
  apiKeys: CLI_PARITY_PROVIDER_KEY_FLAG,
  environment: "--config",
  runtime: "--runtime-size",
  overrides: "--session-timeout",
  webhook: "--webhook",
  message: "--prompt",
  messageIdempotencyKey: "--idempotency-key",
  fileCapture: CLI_PARITY_NOT_SURFACED,
  includeBuiltinTools: CLI_PARITY_NOT_SURFACED,
  outputMode: CLI_PARITY_NOT_SURFACED,
  responseFormat: CLI_PARITY_NOT_SURFACED,
  approvalGate: CLI_PARITY_NOT_SURFACED,
  deleteAfter: CLI_PARITY_NOT_SURFACED,
  stream: CLI_PARITY_NOT_SURFACED
} satisfies Record<keyof SessionStartOptions, string>;

const FILES_COVERAGE = {
  list: CLI_PARITY_BARE_LIST,
  read: "read",
  download: "download",
  link: "link",
  find: "find",
  search: "search",
  last: CLI_PARITY_NOT_SURFACED,
  first: CLI_PARITY_NOT_SURFACED,
  findOne: CLI_PARITY_NOT_SURFACED,
  fetch: CLI_PARITY_NOT_SURFACED
} satisfies Record<keyof SessionFiles, string>;

/** Public `Aex` client methods, reflected off the prototype (drop ctor + internals). */
function publicAexMethods(): string[] {
  return Object.getOwnPropertyNames(Aex.prototype)
    .filter((name) => name !== "constructor" && !name.startsWith("_"))
    .sort();
}

describe("CLI ↔ SDK parity manifest", () => {
  it("covers every SessionStartOptions key (compile-time) and pins the published manifest", () => {
    expect(CLI_SDK_PARITY_MANIFEST.sessionOptionFlags).toEqual(SESSION_OPTION_COVERAGE);
  });

  it("covers every SessionFiles accessor method (compile-time) and pins the published manifest", () => {
    expect(CLI_SDK_PARITY_MANIFEST.filesSubverbs).toEqual(FILES_COVERAGE);
  });

  it("accounts for every public Aex client method — a new SDK method fails until mapped", () => {
    const reflected = publicAexMethods();
    const manifest = Object.keys(CLI_SDK_PARITY_MANIFEST.aexMethods).sort();
    expect(manifest).toEqual(reflected);
  });

  it("maps every Aex method to a REGISTERED CLI verb", () => {
    for (const [method, verb] of Object.entries(CLI_SDK_PARITY_MANIFEST.aexMethods)) {
      expect(CLI_VERB_NAMES, `Aex.${method} → CLI verb "${verb}"`).toContain(verb);
      expect(findVerbSpec(verb), `verb "${verb}" spec`).toBeDefined();
    }
  });

  it("maps every surfaced session-option to a REGISTERED `aex start` flag", () => {
    for (const [key, flag] of Object.entries(CLI_SDK_PARITY_MANIFEST.sessionOptionFlags)) {
      if (flag === CLI_PARITY_NOT_SURFACED || flag === CLI_PARITY_PROVIDER_KEY_FLAG) continue;
      expect(SESSION_FLAGS, `session-option "${key}" → flag "${flag}"`).toContain(flag);
    }
  });

  it("maps every surfaced files accessor to a REGISTERED `aex files` sub-verb", () => {
    for (const [key, sub] of Object.entries(CLI_SDK_PARITY_MANIFEST.filesSubverbs)) {
      if (sub === CLI_PARITY_NOT_SURFACED || sub === CLI_PARITY_BARE_LIST) continue;
      expect(FILES_SUBVERBS, `files.${key} → sub-verb "${sub}"`).toContain(sub);
    }
  });

  it("every registered `aex files` sub-verb is reachable from an SDK accessor method", () => {
    const surfaced = new Set(Object.values(CLI_SDK_PARITY_MANIFEST.filesSubverbs));
    for (const sub of FILES_SUBVERBS) {
      expect(surfaced, `sub-verb "${sub}" has an SDK accessor`).toContain(sub);
    }
  });

  it("the `start` and `files` verbs declare the parity-mapped flags/sub-verbs", () => {
    const start = findVerbSpec("start");
    expect(start?.flags).toEqual(SESSION_FLAGS);
    const files = findVerbSpec("files");
    expect(files?.subverbs).toEqual([...FILES_SUBVERBS]);
  });
});
