/**
 * Live scenario: live-sdk-skill-invocation.test.ts
 *
 * Matrix test — same assertion body across managed provider cells.
 * Proves the agent ACTUALLY FOLLOWS skill content end-to-end, not just
 * that the skill bundle was materialized:
 *
 *   SDK → POST /api/sessions (with inline skills wired)
 *      → preflight uploads skill to object storage
 *      → manifest mounts the skill so the model sees its SKILL.md
 *      → user prompt contains the skill's trigger token (SHIBBOLETH)
 *      → model emits the per-case unique reply the skill demanded
 *
 * Two skills wired per case — alpha is canonical (holds the answer), beta
 * is a distractor. Catches the failure mode where the manifest is read
 * but skill *content* is dropped, and the failure mode where ALL skills
 * collapse into one (model would echo distractor text too).
 *
 * The assertion body runs on a single managed cell:
 *   - (deepseek, managed)  — managed runtime (object storage download)
 *
 * Required env:
 *   AEX_API_URL              live hosted API URL
 *   AEX_API_KEY             workspace API key
 *   DEEPSEEK_API_KEY    customer DeepSeek key
 *   AEX_USER_TEST_TARBALL          packed SDK tarball
 *     OR AEX_USER_TEST_VERSION     published package version
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value || value.length === 0) {
    throw new Error(`user-tests live (skill-invocation): required env ${name} is missing.`);
  }
  return value;
}

const apiUrl = requireEnv("AEX_API_URL");
const apiKey = requireEnv("AEX_API_KEY");
const deepseekKey = requireEnv("DEEPSEEK_API_KEY");
const deepseekModel = process.env["AEX_USER_TEST_DEEPSEEK_MODEL"]?.trim() || "deepseek-v4-flash";

interface Cell {
  readonly id: string;
  readonly provider: "deepseek";
  readonly model: string;
  readonly keyEnvName: string;
  readonly keyValue: string;
}

const CELLS: readonly Cell[] = [
  { id: "deepseek-managed",  provider: "deepseek", model: deepseekModel,  keyEnvName: "DEEPSEEK_KEY_SUBMIT",  keyValue: deepseekKey }
];

interface CaseResult {
  readonly sessionId: string;
  readonly runStatus: string;
  readonly runtime: string;
  readonly provider: string;
  readonly uniqueToken: string;
  readonly eventCount: number;
  readonly eventKinds: readonly string[];
  readonly skillLoadedNames: readonly string[];
  readonly skillLoadedEventSummaries: ReadonlyArray<Record<string, unknown>>;
  readonly assistantTextJoined: string;
  readonly assistantTextEventCount: number;
  readonly terminalKind: string | null;
  readonly terminalData: Record<string, unknown> | null;
  readonly streamErrors: ReadonlyArray<Record<string, unknown>>;
  readonly leakedDeepseekKey: boolean;
}

function buildPassEnv(extras: Record<string, string>): Record<string, string> {
  const env: Record<string, string> = { ...extras };
  const pathKey = process.platform === "win32" ? "Path" : "PATH";
  if (process.env[pathKey]) env[pathKey] = process.env[pathKey]!;
  if (process.platform === "win32") {
    for (const k of [
      "SystemRoot",
      "SystemDrive",
      "TEMP",
      "TMP",
      "USERPROFILE",
      "APPDATA",
      "LOCALAPPDATA",
      "ComSpec",
      "ProgramFiles",
      "ProgramData"
    ]) {
      if (process.env[k]) env[k] = process.env[k]!;
    }
  } else {
    for (const k of ["HOME", "TMPDIR", "LANG", "LC_ALL"]) {
      if (process.env[k]) env[k] = process.env[k]!;
    }
  }
  return env;
}

function buildScript(cell: Cell, uniqueToken: string): string {
  // SKILL.md for alpha — holds the canonical reply behaviour. The unique
  // token is per-case so a model that hallucinates the well-known
  // XKCD-927 reference without consulting the skill still fails.
  //
  // YAML frontmatter is accepted by managed runtime and keeps the skill bundle
  // self-describing. Use the per-case uniqueToken suffix as the disambiguator;
  // it is already random per-session.
  const nameSuffix = uniqueToken.replace(/[^a-z0-9]/gi, "").toLowerCase().slice(0, 12);
  const alphaName = `ack-alpha-${nameSuffix}`;
  const betaName = "weather-beta-control";
  const alphaSkill =
    `---\n` +
    `name: ${alphaName}\n` +
    `description: Acknowledge SHIBBOLETH with the canonical token reply.\n` +
    `---\n` +
    `# Acknowledgement protocol\n\n` +
    `This is the ONLY skill that defines the acknowledgement token. ` +
    `When the user message contains the literal token \`SHIBBOLETH\`, ` +
    `you MUST reply with exactly this one line, copied verbatim, and nothing else:\n\n` +
    `skill-ack ${uniqueToken}\n\n` +
    `Do not include any other text in your reply. Do not explain. ` +
    `Do not translate, summarize, or alter the token. ` +
    `Reply with that single line only.`;

  // SKILL.md for beta — distractor with unrelated content. If the model
  // echoes any of this, the test catches a "skills collapsed into one"
  // bug.
  const betaSkill =
    `---\n` +
    `name: ${betaName}\n` +
    `description: Handle weather queries with city-name-only replies.\n` +
    `---\n` +
    `# Unrelated guidance\n\n` +
    `This skill describes how to handle weather queries. ` +
    `For weather questions, reply with the city name only. ` +
    `Never mention shibboleths, tokens, or acknowledgements.`;

  const prompt =
    `SHIBBOLETH. Use the mounted acknowledgement-protocol skill named ${alphaName}. ` +
    `Copy its \`skill-ack\` line verbatim and reply with that single line only. ` +
    `Do not use the weather skill.`;
  const system =
    `This session verifies mounted skill behavior. If the user asks for the ` +
    `acknowledgement protocol, rely on the mounted skill named ${alphaName} and ` +
    `copy its canonical reply line exactly. Do not answer from general memory.`;

  return `
    import { Aex, Skill } from "@aexhq/sdk";

    const client = new Aex({
      baseUrl: process.env.AEX_API_URL,
      apiKey: process.env.AEX_API_KEY
    });

    const alpha = await Skill.fromContent(${JSON.stringify(alphaSkill)}, {
      name: ${JSON.stringify(alphaName)}
    });
    const beta = await Skill.fromContent(${JSON.stringify(betaSkill)}, {
      name: ${JSON.stringify(betaName)}
    });
    const alphaRef = await client.workspace.skills.publish(alpha);
    const betaRef = await client.workspace.skills.publish(beta);

    const sessionResult = await client.start({
      provider: ${JSON.stringify(cell.provider)},
      model: ${JSON.stringify(cell.model)},
      system: ${JSON.stringify(system)},
      message: ${JSON.stringify(prompt)},
      assets: { skills: [alphaRef, betaRef] },
      apiKeys: { [${JSON.stringify(cell.provider)}]: process.env.${cell.keyEnvName} },
      idempotencyKey: "skill-invocation-${cell.id}-" + Date.now()
    }, { timeoutMs: 6 * 60_000 });
    const sessionId = sessionResult.sessionId;
    const run = {
      status: sessionResult.status,
      runtime: "managed",
      provider: ${JSON.stringify(cell.provider)}
    };

    const session = await client.sessions.open(sessionId);
    const events = (await session.events.list()).filter((event) => event.runId === sessionResult.run.runId);

    // CUSTOM envelopes nest the original payload under data.value, keyed by
    // data.name (aex.notification / aex.skill_loaded / aex.stream_error).
    function customName(e) {
      return e && e.data && typeof e.data.name === "string" ? e.data.name : null;
    }
    function customValue(e) {
      const value = e && e.data ? e.data.value : null;
      return value && typeof value === "object" ? value : {};
    }
    function skillLoadedName(e) {
      const value = customValue(e);
      if (
        customName(e) === "aex.skill_loaded" ||
        value.kind === "skill_loaded" ||
        value.kind === "skill_loaded_marker"
      ) {
        const name = value.name || value.skillId;
        return typeof name === "string" ? name : null;
      }
      return null;
    }
    function skillLoadedSummary(e) {
      const value = customValue(e);
      return {
        customName: customName(e),
        kind: typeof value.kind === "string" ? value.kind : null,
        name: typeof value.name === "string" ? value.name : null,
        skillId: typeof value.skillId === "string" ? value.skillId : null
      };
    }
    const customEvents = events.filter((e) => e.type === "CUSTOM");
    const skillLoadedEventSummaries = customEvents
      .filter((n) => skillLoadedName(n))
      .map(skillLoadedSummary);
    const skillLoadedNames = customEvents.map(skillLoadedName).filter(Boolean);

    const assistantTextEvents = events.filter((e) => e.type === "TEXT_MESSAGE_CONTENT");
    const assistantTextJoined = assistantTextEvents
      .map((e) => (e.data && typeof e.data.text === "string" ? e.data.text : ""))
      .join(" ");

    const terminal = events.find((e) => e.type === "RUN_FINISHED" || e.type === "RUN_ERROR");
    const eventKinds = events.map((e) => e.type);
    const terminalData = terminal ? terminal.data : null;
    const streamErrors = customEvents
      .filter((e) => e.data && e.data.name === "aex.stream_error")
      .map((e) => (e.data.value && typeof e.data.value === "object" ? e.data.value : { unknown: true }));

    const serialized = JSON.stringify({ run, events });
    const deepseekEnv = process.env.DEEPSEEK_KEY ?? "";
    const result = {
      sessionId: sessionId,
      runStatus: run.status,
      runtime: run.runtime ?? "(missing)",
      provider: run.provider ?? "(missing)",
      uniqueToken: ${JSON.stringify(uniqueToken)},
      eventCount: events.length,
      eventKinds,
      skillLoadedNames,
      skillLoadedEventSummaries,
      assistantTextJoined,
      assistantTextEventCount: assistantTextEvents.length,
      terminalKind: terminal ? terminal.type : null,
      terminalData,
      streamErrors,
      leakedDeepseekKey: deepseekEnv.length > 0 && serialized.includes(deepseekEnv)
    };
    process.stdout.write(JSON.stringify(result));
    process.exit(0);
  `;
}

function dumpResult(cell: Cell, result: CaseResult): string {
  const lines: string[] = [];
  lines.push(`cell=${cell.id} sessionId=${result.sessionId}`);
  lines.push(`runStatus=${result.runStatus} runtime=${result.runtime} provider=${result.provider}`);
  lines.push(`uniqueToken=${result.uniqueToken}`);
  lines.push(`terminalKind=${result.terminalKind} terminalData=${JSON.stringify(result.terminalData)}`);
  lines.push(`eventKinds=[${result.eventKinds.join(", ")}]`);
  lines.push(`skillLoadedNames=[${result.skillLoadedNames.join(", ")}]`);
  lines.push(`skillLoadedEventSummaries=${JSON.stringify(result.skillLoadedEventSummaries)}`);
  if (result.streamErrors.length > 0) {
    lines.push(`streamErrors:`);
    for (const se of result.streamErrors) {
      lines.push(`  - ${JSON.stringify(se).slice(0, 600)}`);
    }
  }
  lines.push(`assistantText=${result.assistantTextJoined.slice(0, 800)}`);
  return lines.join("\n");
}

async function runCell(cell: Cell, installDir: string, uniqueToken: string): Promise<CaseResult> {
  const script = buildScript(cell, uniqueToken);
  const scriptPath = join(installDir, `skill-invocation-${cell.id}.mjs`);
  writeFileSync(scriptPath, script);
  const passEnv = buildPassEnv({
    AEX_API_URL: apiUrl,
    AEX_API_KEY: apiKey,
    [cell.keyEnvName]: cell.keyValue,
    DEEPSEEK_KEY: deepseekKey
  });
  const child = await runCommand(getBunCommand(), [scriptPath], {
    cwd: installDir,
    timeoutMs: 8 * 60_000,
    env: passEnv
  });
  if (child.exitCode !== 0) {
    throw new Error(
      `skill-invocation runner (${cell.id}) exited non-zero (${child.exitCode}):\n--- stdout ---\n${child.stdout}\n--- stderr ---\n${child.stderr}`
    );
  }
  return JSON.parse(child.stdout.trim()) as CaseResult;
}

let install: InstallResult;

beforeAll(async () => {
  install = await installAex();
}, 240_000);

afterAll(() => {
  install?.cleanup();
});

describe("live skill invocation — agent actually follows skill content", () => {
  it.each([...CELLS])(
    "$id: SKILL.md drives reply (canonical alpha, distractor beta)",
    async (cell) => {
      // XKCD-927-<random> per case so model recall of the well-known
      // joke is not enough — the model must read this session's skill.
      const uniqueToken = "XKCD-927-" + Math.random().toString(36).slice(2, 10).toUpperCase();
      const result = await runCell(cell, install.installDir, uniqueToken);
      const dump = (): string => dumpResult(cell, result);
      const nameSuffix = uniqueToken.replace(/[^a-z0-9]/gi, "").toLowerCase().slice(0, 12);
      const expectedSkillPrefixes = [`ack-alpha-${nameSuffix}`, "weather-beta-control"];

      expect(result.runStatus, dump()).toBe("succeeded");
      expect(result.runtime).toBe("managed");
      expect(result.provider).toBe(cell.provider);

      // The committed public terminal is the only completion barrier.
      expect(result.terminalKind).toBe("RUN_FINISHED");
      expect(result.terminalData?.["outcome"], dump()).toBe("succeeded");

      expect(result.skillLoadedNames.length, dump()).toBeGreaterThanOrEqual(2);
      for (const prefix of expectedSkillPrefixes) {
        if (!result.skillLoadedNames.some((n) => n.startsWith(prefix))) {
          throw new Error(`skill "${prefix}" produced no skill_loaded event\n\n${dump()}`);
        }
      }

      // The MODEL actually applied alpha's content — the per-case unique
      // token is present in the assistant text. Stripping whitespace so
      // streaming token boundaries don't break the match.
      //
      // Reply directive is `skill-ack <uniqueToken>`, NOT `skill-token=...`.
      // The stream-before-disk redactor masks
      // any `token`/`key`/`secret`-keyworded `key<sep>value` run AND any
      // high-entropy [A-Za-z0-9+/=-]{24,} run. The old `skill-token=<tok>`
      // tripped BOTH (the literal word "token" + the `=`-glued blob), so
      // the canonical reply was redacted to `skill-[REDACTED]` in managed-runtime
      // stdout before the event stream was built and never matched. A
      // space-separated, keyword-free `skill-ack <tok>` survives, and the
      // 17-char `XKCD-927-…` token survives standalone (sub-24-char).
      const normalized = result.assistantTextJoined.replace(/\s+/g, "");
      const expected = `skill-ack ${uniqueToken}`;
      const expectedNormalized = expected.replace(/\s+/g, "");
      if (!normalized.includes(expectedNormalized)) {
        throw new Error(
          `assistant_text missing canonical reply "${expected}"\n\n${dump()}`
        );
      }

      expect(result.assistantTextEventCount).toBeGreaterThan(0);
      expect(result.leakedDeepseekKey, dump()).toBe(false);
    },
    10 * 60_000
  );
});
