import { describe, expect, it } from "bun:test";
import { parsePostHook } from "../src/post-hook.js";

/**
 * `parsePostHook` treats a blank command as "no hook", and reaches that answer
 * BEFORE looking at `timeout`, `maxTurns` or `maxChars`. That ordering is the
 * whole point of the rule — callers pre-fill a hook config with placeholder
 * values and leave the command empty until they want it to run.
 *
 * It had no test. The schema port initially validated in one pass and started
 * rejecting exactly the configs the rule exists to allow, which is what these
 * cases now prevent.
 */
describe("parsePostHook blank-command short-circuit", () => {
  it.each([
    ["whitespace command alone", { command: "   " }],
    ["empty command alone", { command: "" }],
    ["blank command beside an unparseable timeout", { command: "  ", timeout: 5 }],
    ["blank command beside an invalid maxTurns", { command: "", maxTurns: "nope" }],
    ["blank command beside an invalid maxChars", { command: " ", maxChars: -1 }]
  ])("returns undefined for a %s", (_name, input) => {
    expect(parsePostHook(input, "submission.postHook")).toBeUndefined();
  });

  it("still rejects a sibling field once the command is real", () => {
    expect(() => parsePostHook({ command: "echo", timeout: 5 }, "submission.postHook")).toThrow(
      'submission.postHook.timeout must be a duration string (e.g. "5m", "30s"); got 5'
    );
  });

  it("still rejects an unknown key even when the command is blank", () => {
    expect(() => parsePostHook({ command: "", nope: 1 }, "submission.postHook")).toThrow(
      "submission.postHook.nope is not an allowed field; permitted: command, timeout, maxTurns, maxChars"
    );
  });

  it("applies defaults for a real command", () => {
    expect(parsePostHook({ command: "echo" })).toEqual({
      command: "echo",
      timeoutMs: 300_000,
      maxTurns: 10,
      maxChars: null
    });
  });
});
