/**
 * Offline coverage of the `client.start(...)` option gate, through a clean
 * installed `@aexhq/sdk` (blackbox, child process, cwd = install tempdir).
 *
 * This was case 0 of `test/live/edge-session-lifecycle.user.test.ts`, labelled
 * there as "pure CLIENT-side validation (no HTTP, no billable session turn)" and
 * already proving it with an injected fetch and an `httpCalls === 0` assertion.
 * A case that asserts the network was never touched has no business consuming a
 * live runtime-matrix job; split out 2026-07-27.
 *
 * `detailsOnlyField` is asserted so the error carries a stable, machine-readable
 * `{ field }` and nothing else — a caller can branch on it without parsing prose.
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";

interface RejectionInfo {
  readonly name: string | null;
  readonly hasMessage: boolean;
  readonly status: number | null;
  readonly code: string | null;
  readonly detailsField: string | null;
  readonly detailsOnlyField: boolean;
}

const SCRIPT = String.raw`
import { Aex } from "@aexhq/sdk";

const MODEL = "deepseek/deepseek-v4-flash";
let httpCalls = 0;
const client = new Aex({
  baseUrl: "https://example.invalid",
  apiKey: "aex_offline_session_options",
  retry: false,
  fetch: async () => { httpCalls += 1; throw new Error("CLIENT_VALIDATION_MADE_HTTP"); }
});

function errInfo(e) {
  const details = e && e.details && typeof e.details === "object" && !Array.isArray(e.details) ? e.details : null;
  const detailKeys = details ? Object.keys(details) : [];
  return {
    name: e && e.name ? e.name : null,
    hasMessage: !!(e && e.message),
    status: (e && typeof e.status === "number") ? e.status : null,
    code: (e && e.code) ? e.code : null,
    detailsField: details && typeof details.field === "string" ? details.field : null,
    detailsOnlyField: detailKeys.length === 1 && detailKeys[0] === "field"
  };
}

async function rej(fn) {
  try {
    await fn();
    return { name: "Resolved", hasMessage: false, status: null, code: null, detailsField: null, detailsOnlyField: false };
  } catch (e) {
    return errInfo(e);
  }
}

const emptyMsg = await rej(() => client.start({ model: MODEL, message: "" }));
const emptyArr = await rej(() => client.start({ model: MODEL, message: [] }));
const emptySegment = await rej(() => client.start({ model: MODEL, message: ["ok", ""] }));
const whitespaceMsg = await rej(() => client.start({ model: MODEL, message: "  \n\t " }));
const whitespaceArray = await rej(() => client.start({ model: MODEL, message: ["  ", "\n"] }));
const legacyPrompt = await rej(() => client.start({ model: MODEL, message: "hi", prompt: "x" }));
const unknownOption = await rej(() => client.start({ model: MODEL, message: "hi", totallyUnknownOption: { nope: true } }));

process.stdout.write(JSON.stringify({
  emptyMsg, emptyArr, emptySegment, whitespaceMsg, whitespaceArray, legacyPrompt, unknownOption, httpCalls
}));
`;

describe("installed SDK session-option client-side gate", () => {
  let install: InstallResult;
  let result: Record<string, RejectionInfo> & { httpCalls: number };

  beforeAll(async () => {
    install = await installAex();
    const path = join(install.installDir, "session-options-validation.mjs");
    writeFileSync(path, SCRIPT);
    const child = await runCommand(getBunCommand(), [path], {
      cwd: install.installDir,
      timeoutMs: 120_000
    });
    if (child.exitCode !== 0) {
      throw new Error(`session-options-validation.mjs exited ${child.exitCode}\n${child.stderr}`);
    }
    result = JSON.parse(child.stdout.trim());
  }, 300_000);

  afterAll(() => install?.cleanup());

  function expectConfigError(key: string, field: string): void {
    const value = result[key];
    expect(value, `${key} verdict is missing`).toBeDefined();
    // Captured before the assertion: bun 1.3.14's toMatchObject mutates the
    // received object, so a later read of `value` would see erased fields.
    const dump = JSON.stringify(value);
    expect(value, dump).toMatchObject({
      name: "SessionConfigValidationError",
      code: "SESSION_CONFIG_INVALID",
      hasMessage: true,
      detailsField: field,
      detailsOnlyField: true
    });
  }

  it("rejects an empty, whitespace-only, or empty-segment message", () => {
    expectConfigError("emptyMsg", "message");
    expectConfigError("emptyArr", "message");
    expectConfigError("emptySegment", "message");
    expectConfigError("whitespaceMsg", "message");
    expectConfigError("whitespaceArray", "message");
  });

  it("rejects the removed `prompt` field and any unknown option by name", () => {
    expectConfigError("legacyPrompt", "prompt");
    expectConfigError("unknownOption", "totallyUnknownOption");
  });

  it("issues no HTTP request for any rejected option", () => {
    expect(result.httpCalls).toBe(0);
  });
});
