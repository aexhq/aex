/**
 * USER TEST (SDK-driven) — postHook runs after managed agent completion.
 *
 * Validates the deployed public surface through the installed SDK. These cases
 * intentionally inspect only user-visible run state and event envelopes:
 *   - a passing hook succeeds the run;
 *   - a failing hook triggers one repair turn, then passes on retry;
 *   - maxTurns:0 fails terminally, and maxChars caps repair feedback.
 */
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { installAex, type InstallResult } from "../_fixtures/install.js";
import { requireUserEnv, runSdkScript, sdkRunnerScript, type SdkRunResult } from "./_sdk.js";

const env = requireUserEnv({ deepseek: true });

interface PostHookCase {
  readonly id: string;
  readonly command: string;
  readonly timeout?: string;
  readonly maxTurns?: number;
  readonly maxChars?: number | null;
}

function postHookLiteral(spec: PostHookCase): string {
  const fields = [`command: ${JSON.stringify(spec.command)}`];
  if (spec.timeout !== undefined) fields.push(`timeout: ${JSON.stringify(spec.timeout)}`);
  if (spec.maxTurns !== undefined) fields.push(`maxTurns: ${spec.maxTurns}`);
  if (spec.maxChars !== undefined) fields.push(`maxChars: ${spec.maxChars === null ? "null" : spec.maxChars}`);
  return `{ ${fields.join(", ")} }`;
}

function buildScript(spec: PostHookCase): string {
  return sdkRunnerScript({
    submit: `{
      provider: "deepseek",
      runtime: "managed",
      model: MODEL_DEEPSEEK,
      prompt: "Reply with exactly: post hook ready.",
      builtins: [],
      postHook: ${postHookLiteral(spec)},
      secrets: { apiKey: DEEPSEEK_KEY },
      idempotencyKey: "user-posthook-${spec.id}-" + Date.now()
    }`
  });
}

async function runCase(install: InstallResult, spec: PostHookCase): Promise<SdkRunResult> {
  return await runSdkScript(install, env, buildScript(spec), {
    scriptName: `user-posthook-${spec.id}.mjs`,
    waitMs: spec.id === "retry" ? 10 * 60_000 : 6 * 60_000,
    timeoutMs: spec.id === "retry" ? 11 * 60_000 : 7 * 60_000
  });
}

function asRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object" && !Array.isArray(value) ? (value as Record<string, unknown>) : null;
}

function dumpResult(result: SdkRunResult): string {
  return [
    `runId=${result.runId}`,
    `status=${result.status} runtime=${result.runtime} provider=${result.provider}`,
    `terminalKind=${result.terminalKind} terminalData=${JSON.stringify(result.terminalData)}`,
    `eventKinds=[${result.eventKinds.join(", ")}]`,
    `streamErrors=${JSON.stringify(result.streamErrors).slice(0, 1000)}`,
    `assistantText=${result.assistantText.slice(0, 400)}`
  ].join("\n");
}

function requirePostHookSummary(result: SdkRunResult): Record<string, unknown> {
  const dump = dumpResult(result);
  const terminal = asRecord(result.terminalData);
  if (!terminal) throw new Error(`terminalData is missing\n\n${dump}`);
  const postHook = asRecord(terminal["postHook"]);
  if (!postHook) throw new Error(`terminalData.postHook is missing\n\n${dump}`);
  return postHook;
}

function requireFinalHookResult(summary: Record<string, unknown>, result: SdkRunResult): Record<string, unknown> {
  const finalResult = asRecord(summary["finalResult"]);
  if (!finalResult) {
    throw new Error(`terminalData.postHook.finalResult is missing\n\n${dumpResult(result)}`);
  }
  return finalResult;
}

function postHookFailures(result: SdkRunResult): ReadonlyArray<Record<string, unknown>> {
  return result.streamErrors.filter((event) => event["type"] === "post_agent_run_hook_failed");
}

let install: InstallResult;

beforeAll(async () => {
  install = await installAex();
}, 240_000);

afterAll(() => {
  install?.cleanup();
});

describe("user/SDK: postHook verifies managed runs after agent completion", () => {
  it(
    "passes when the hook exits 0",
    async () => {
      const result = await runCase(install, {
        id: "pass",
        command: 'test "$(pwd)" = "/workspace"',
        timeout: "45s",
        maxTurns: 0
      });
      const dump = (): string => dumpResult(result);

      expect(result.status, dump()).toBe("succeeded");
      expect(result.runtime).toBe("managed");
      expect(result.terminalKind).toBe("RUN_FINISHED");
      expect(result.terminalData?.["reason"], dump()).toBe("complete");

      const summary = requirePostHookSummary(result);
      expect(summary["attempts"], dump()).toBe(1);
      expect(summary["repairTurns"], dump()).toBe(0);
      const finalResult = requireFinalHookResult(summary, result);
      expect(finalResult["exitCode"], dump()).toBe(0);
      expect(finalResult["timedOut"], dump()).toBe(false);
      expect(postHookFailures(result), dump()).toHaveLength(0);
    },
    8 * 60_000
  );

  it(
    "reruns the hook after one structured repair turn",
    async () => {
      const result = await runCase(install, {
        id: "retry",
        command:
          "if [ -f .post-hook-retry-state ]; then echo retry-pass; exit 0; fi; " +
          "echo retry-first-failure; touch .post-hook-retry-state; exit 1",
        timeout: "45s",
        maxTurns: 1,
        maxChars: null
      });
      const dump = (): string => dumpResult(result);

      expect(result.status, dump()).toBe("succeeded");
      expect(result.terminalKind).toBe("RUN_FINISHED");
      expect(result.terminalData?.["reason"], dump()).toBe("complete");

      const summary = requirePostHookSummary(result);
      expect(summary["attempts"], dump()).toBe(2);
      expect(summary["repairTurns"], dump()).toBe(1);
      const finalResult = requireFinalHookResult(summary, result);
      expect(finalResult["exitCode"], dump()).toBe(0);

      const failures = postHookFailures(result);
      expect(failures, dump()).toHaveLength(1);
      expect(failures[0]?.["attempt"], dump()).toBe(1);
      const output = asRecord(failures[0]?.["output"]);
      expect(output?.["stdout"], dump()).toContain("retry-first-failure");
      expect(output?.["truncated"], dump()).toBe(false);
    },
    12 * 60_000
  );

  it(
    "fails terminally when maxTurns is exhausted and caps hook output with maxChars",
    async () => {
      const result = await runCase(install, {
        id: "maxchars",
        command: "printf ABCDEFGHIJ; printf KLMNOPQRST >&2; exit 1",
        timeout: "45s",
        maxTurns: 0,
        maxChars: 4
      });
      const dump = (): string => dumpResult(result);

      expect(result.status, dump()).toBe("failed");
      expect(result.terminalData?.["reason"], dump()).toBe("post_hook_failed");

      const summary = requirePostHookSummary(result);
      expect(summary["attempts"], dump()).toBe(1);
      expect(summary["repairTurns"], dump()).toBe(0);
      const finalResult = requireFinalHookResult(summary, result);
      expect(finalResult["exitCode"], dump()).toBe(1);

      const failures = postHookFailures(result);
      expect(failures, dump()).toHaveLength(1);
      const failure = failures[0]!;
      const hook = asRecord(failure["hook"]);
      const output = asRecord(failure["output"]);
      const hookResult = asRecord(failure["result"]);

      expect(hook?.["timeoutMs"], dump()).toBe(45_000);
      expect(hookResult?.["exitCode"], dump()).toBe(1);
      expect(output?.["stdout"], dump()).toBe("ABCD");
      expect(output?.["stderr"], dump()).toBe("KLMN");
      expect(output?.["truncated"], dump()).toBe(true);
    },
    8 * 60_000
  );
});
