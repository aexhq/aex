/**
 * Live scenario: live-skill-tool-staging.test.ts
 *
 * Exercises the first-class skill filesystem-staging contract end-to-end
 * against the deployed hosted API, from the perspective of a real SDK user. A
 * skill bundle (built with `Skill.fromDir`) is uploaded/upserted by workspace
 * name; at execution time the managed runtime EAGERLY stages every file in the bundle
 * under `/workspace/skills/<name>/`, and the model calls the `skills` meta-tool
 * to pull the `SKILL.md` body into context. The platform-side load handler:
 *   - reads `<skillDir>/SKILL.md`,
 *   - byte-caps the returned body at `HANDS_SKILL_MD_MAX_BYTES = 400_000`
 *     (appending `\n[skill SKILL.md truncated at 400000 bytes]` when it cut),
 *   - sessions it through the shape-based secret redactor before returning it.
 *
 * Four independent live sessions, each asserting only via OBSERVABLE session evidence
 * (assistant text + the event stream, including TOOL_CALL_RESULT `data.content`
 * — the verbatim tool result). All assertions key off PLANTED DETERMINISTIC
 * TOKENS (never free-form model phrasing), so they survive LLM nondeterminism.
 *
 *   1. Files staged WITH DIRECTORY STRUCTURE — bundle carries `SKILL.md` plus
 *      `data/payload.txt` holding a planted token; the agent reads that exact
 *      path with a builtin file tool. Proves the WHOLE bundle (not just
 *      SKILL.md) staged, subdirectory intact, at `/workspace/skills/<name>/`.
 *   2. `skills` load returns the SKILL.md BODY — a token planted only in the
 *      SKILL.md prose reaches the model when it loads the skill.
 *   3. Byte-cap — a SKILL.md whose body is ~460 KB (a near-start marker plus a
 *      marker placed PAST the 400_000-byte boundary). The near-start marker
 *      loads; the beyond-cap marker never appears anywhere in the session.
 *   4. Secret redaction — a secret-SHAPED value (`sk-ant-…`) in the SKILL.md
 *      body is returned as `[REDACTED]`, never verbatim.
 *
 * Gating: this is a LIVE suite. Missing creds (AEX_API_URL / AEX_API_KEY /
 * DEEPSEEK_API_KEY) are a hard collection-time failure; the live lane must never
 * pass by silently skipping.
 *
 * Required env (live only):
 *   AEX_API_URL                 live hosted API URL
 *   AEX_API_KEY               workspace API key
 *   DEEPSEEK_API_KEY            customer DeepSeek key
 *   AEX_USER_TEST_TARBALL       packed SDK tarball
 *     OR AEX_USER_TEST_VERSION  published package version
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";

const apiUrl = requireEnv("AEX_API_URL");
const apiKey = requireEnv("AEX_API_KEY");
const deepseekKey = requireEnv("DEEPSEEK_API_KEY");
const deepseekModel = process.env["AEX_USER_TEST_DEEPSEEK_MODEL"]?.trim() || "deepseek-v4-flash";

function requireEnv(name: string): string {
  const value = process.env[name]?.trim();
  if (!value) {
    throw new Error(`${name} is required for live skill staging tests.`);
  }
  return value;
}

interface SkillFileSpec {
  /** POSIX-relative path inside the bundle (e.g. "SKILL.md", "data/payload.txt"). */
  readonly path: string;
  readonly content: string;
}

interface ScriptConfig {
  readonly skillName: string;
  readonly files: readonly SkillFileSpec[];
  readonly system: string;
  readonly message: string;
  /**
   * A JS object-literal expression (source text) computing case-specific boolean
   * checks. Evaluated inside the child sessionner, where these vars are in scope:
   *   assistantTextJoined / assistantTextNorm  — joined TEXT_MESSAGE_CONTENT
   *   toolResultsJoined    / toolResultsNorm   — JSON of every TOOL_CALL_RESULT
   *   haystack             / haystackNorm      — JSON of all events + files
   * (`*Norm` variants are whitespace-stripped so streamed token boundaries and
   * model-inserted spaces never break a substring match on a planted token.)
   */
  readonly checksExpr: string;
  readonly idempotencyPrefix: string;
}

interface CaseResult {
  readonly sessionId: string;
  readonly sessionStatus: string;
  readonly ok: boolean;
  readonly terminalKind: string | null;
  readonly terminalReason: unknown;
  readonly eventKinds: readonly string[];
  readonly skillLoadedNames: readonly string[];
  readonly assistantTextExcerpt: string;
  readonly toolResultsExcerpt: string;
  readonly streamErrors: ReadonlyArray<Record<string, unknown>>;
  readonly checks: Record<string, boolean>;
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

/** A short, redaction-safe planted token: `PREFIX-XXXXXXXX`, always < 24 chars so
 *  the high-entropy catch-all (`[A-Za-z0-9+=-]{24,}`) never eats it. */
function tok(prefix: string): string {
  return `${prefix}-${Math.random().toString(36).slice(2, 10).toUpperCase()}`;
}

/** A valid, unique skill name (SKILL_NAME_PATTERN: lowercase kebab/underscore). */
function skillName(base: string): string {
  return `${base}-${Math.random().toString(36).slice(2, 8)}`;
}

function buildScript(cfg: ScriptConfig): string {
  return `
    import { Aex, Skill } from "@aexhq/sdk";
    import { mkdtempSync, mkdirSync, writeFileSync } from "node:fs";
    import { tmpdir } from "node:os";
    import { join, dirname } from "node:path";

    const client = new Aex({
      baseUrl: process.env.AEX_API_URL,
      apiKey: process.env.AEX_API_KEY
    });

    // Materialize the skill bundle on the local FS (SKILL.md + any extra files,
    // including subdirectories) then build a first-class Skill from the directory.
    const skillDir = mkdtempSync(join(tmpdir(), "aex-skilltool-"));
    const filesToStage = ${JSON.stringify(cfg.files)};
    for (const f of filesToStage) {
      const dest = join(skillDir, f.path);
      mkdirSync(dirname(dest), { recursive: true });
      writeFileSync(dest, f.content);
    }
    const skill = await Skill.fromDir(skillDir, { name: ${JSON.stringify(cfg.skillName)} });

    const sessionResult = await client.start({
      provider: "deepseek",
      model: ${JSON.stringify(deepseekModel)},
      system: ${JSON.stringify(cfg.system)},
      message: ${JSON.stringify(cfg.message)},
      skills: [skill],
      apiKeys: { deepseek: process.env.DEEPSEEK_KEY_SUBMIT },
      idempotencyKey: ${JSON.stringify(cfg.idempotencyPrefix)} + "-" + Date.now()
    }, { timeoutMs: 8 * 60_000 });

    const fallbackEvents = Array.isArray(sessionResult.events) ? sessionResult.events : [];
    const fallbackFiles = Array.isArray(sessionResult.files) ? sessionResult.files : [];
    let events = fallbackEvents;
    let files = fallbackFiles;
    try {
      const session = await client.sessions.open(sessionResult.sessionId);
      const listedEvents = await session.events().list();
      if (Array.isArray(listedEvents) && listedEvents.length > 0) events = listedEvents;
      const listedFiles = await session.files().list();
      if (Array.isArray(listedFiles)) files = listedFiles;
    } catch {
      events = fallbackEvents;
      files = fallbackFiles;
    }

    // CUSTOM envelopes nest the original payload under data.value, keyed by data.name.
    function customName(e) {
      return e && e.data && typeof e.data.name === "string" ? e.data.name : null;
    }
    function customValue(e) {
      const v = e && e.data ? e.data.value : null;
      return v && typeof v === "object" ? v : {};
    }
    const SESSION_TERMINAL_NAMES = new Set(["aex.session.idle", "aex.session.suspended", "aex.session.succeeded", "aex.session.failed", "aex.session.timed_out", "aex.session.cancelled"]);
    function isSessionIdle(e) {
      return e && e.type === "CUSTOM" && e.data && SESSION_TERMINAL_NAMES.has(e.data.name);
    }
    function terminalKindOf(e) {
      if (!e) return null;
      return isSessionIdle(e) ? "TURN_FINISHED" : e.type;
    }
    function terminalReasonOf(e) {
      if (!e) return null;
      if (isSessionIdle(e)) {
        const v = customValue(e);
        return v.reason === "completed" ? "complete" : v.reason ?? null;
      }
      return e.data ? e.data.reason : null;
    }
    function skillLoadedName(e) {
      const v = customValue(e);
      if (customName(e) === "aex.skill_loaded" || v.kind === "skill_loaded" || v.kind === "skill_loaded_marker") {
        const n = v.name || v.skillId;
        return typeof n === "string" ? n : null;
      }
      return null;
    }
    const skillLoadedNames = events.filter((e) => e.type === "CUSTOM").map(skillLoadedName).filter(Boolean);

    const assistantTextJoined = events
      .filter((e) => e.type === "TEXT_MESSAGE_CONTENT")
      .map((e) => (e.data && typeof e.data.text === "string" ? e.data.text : ""))
      .join(" ");
    const assistantTextNorm = assistantTextJoined.replace(/\\s+/g, "");

    // The skills meta-tool's returned SKILL.md body (capped + redacted) rides here as a
    // normal TOOL_CALL_RESULT; so does the file-read result in case 1.
    const toolResultsJoined = events
      .filter((e) => e.type === "TOOL_CALL_RESULT")
      .map((e) => { try { return JSON.stringify(e.data); } catch { return ""; } })
      .join("\\n");
    const toolResultsNorm = toolResultsJoined.replace(/\\s+/g, "");

    const eventsJson = JSON.stringify(events);
    const filesJson = JSON.stringify(files);
    const haystack = eventsJson + " " + filesJson;
    const haystackNorm = haystack.replace(/\\s+/g, "");

    const terminal = events.find((e) => e.type === "TURN_FINISHED" || e.type === "TURN_ERROR") ?? events.find(isSessionIdle);
    const eventKinds = events.map((e) => e.type);
    if (terminal && isSessionIdle(terminal) && !eventKinds.includes("TURN_FINISHED")) eventKinds.push("TURN_FINISHED");
    const streamErrors = events
      .filter((e) => e.type === "CUSTOM" && e.data && e.data.name === "aex.stream_error")
      .map((e) => (e.data && typeof e.data.value === "object" && e.data.value ? e.data.value : { unknown: true }));

    const checks = (${cfg.checksExpr});

    process.stdout.write(JSON.stringify({
      sessionId: sessionResult.sessionId,
      sessionStatus: sessionResult.ok ? "succeeded" : (typeof sessionResult.status === "string" && sessionResult.status ? sessionResult.status : "failed"),
      ok: !!sessionResult.ok,
      terminalKind: terminalKindOf(terminal),
      terminalReason: terminalReasonOf(terminal),
      eventKinds,
      skillLoadedNames,
      assistantTextExcerpt: assistantTextJoined.slice(0, 1200),
      toolResultsExcerpt: toolResultsJoined.slice(0, 1200),
      streamErrors,
      checks
    }));
    process.exit(0);
  `;
}

async function runScenario(installDir: string, scriptName: string, cfg: ScriptConfig): Promise<CaseResult> {
  const scriptPath = join(installDir, scriptName);
  writeFileSync(scriptPath, buildScript(cfg));
  const passEnv = buildPassEnv({
    AEX_API_URL: apiUrl,
    AEX_API_KEY: apiKey,
    DEEPSEEK_KEY_SUBMIT: deepseekKey
  });
  const child = await runCommand(getBunCommand(), [scriptPath], {
    cwd: installDir,
    timeoutMs: 10 * 60_000,
    env: passEnv
  });
  if (child.exitCode !== 0) {
    throw new Error(
      `skill-staging runner (${scriptName}) exited non-zero (${child.exitCode}):\n--- stdout ---\n${child.stdout}\n--- stderr ---\n${child.stderr}`
    );
  }
  return JSON.parse(child.stdout.trim()) as CaseResult;
}

function dump(result: CaseResult): string {
  return [
    `sessionId=${result.sessionId} sessionStatus=${result.sessionStatus} ok=${result.ok}`,
    `terminalKind=${result.terminalKind} terminalReason=${JSON.stringify(result.terminalReason)}`,
    `eventKinds=[${result.eventKinds.join(", ")}]`,
    `skillLoadedNames=[${result.skillLoadedNames.join(", ")}]`,
    `checks=${JSON.stringify(result.checks)}`,
    result.streamErrors.length > 0
      ? `streamErrors=${JSON.stringify(result.streamErrors).slice(0, 800)}`
      : "streamErrors=[]",
    `assistantTextExcerpt=${result.assistantTextExcerpt.slice(0, 600)}`,
    `toolResultsExcerpt=${result.toolResultsExcerpt.slice(0, 600)}`
  ].join("\n");
}

function assertSessionOk(result: CaseResult): void {
  expect(result.sessionStatus, dump(result)).toBe("succeeded");
  expect(result.terminalKind, dump(result)).toBe("TURN_FINISHED");
  // Both adapters populate reason="complete" on the clean success path.
  if (result.terminalReason !== "complete") {
    throw new Error(`terminal reason=${JSON.stringify(result.terminalReason)} (expected "complete")\n\n${dump(result)}`);
  }
}

let install: InstallResult;

describe("live skill — filesystem staging + byte-cap + secret redaction", () => {
  beforeAll(async () => {
    install = await installAex();
  }, 240_000);

  afterAll(() => {
    install?.cleanup();
  });

  it(
    "stages the whole bundle (incl. a subdirectory) under /workspace/skills/<name>/",
    async () => {
      const name = skillName("skilltool-staging");
      const token = tok("STAGED");
      const cfg: ScriptConfig = {
        skillName: name,
        files: [
          {
            path: "SKILL.md",
            content:
              `---\nname: ${name}\ndescription: Skill whose bundle also ships a data file under a subdirectory.\n---\n` +
              `# Staging probe\nThis skill's bundle includes an extra file at data/payload.txt.\n`
          },
          // Planted token lives ONLY in a subdirectory file — reaching it proves
          // the entire bundle (not just SKILL.md) staged with structure intact.
          { path: "data/payload.txt", content: `${token}\n` }
        ],
        system: "You are running an automated staging check. Use your file tools; do not answer from memory.",
        message:
          `A skill named ${name} was staged into your container workspace. Its bundle includes an extra file at ` +
          `/workspace/skills/${name}/data/payload.txt. Use the bash tool (e.g. \`cat\`) or the read_file tool to ` +
          `read that exact file, then reply with its exact contents on a single line.`,
        // STAGED-… is < 24 chars ⇒ not touched by the redactor; it appears in the
        // file-read TOOL_CALL_RESULT (⇒ haystack) and/or the model's echo.
        checksExpr: `{ stagedTokenPresent: (haystackNorm.includes(${JSON.stringify(token)}) || assistantTextNorm.includes(${JSON.stringify(token)})) }`,
        idempotencyPrefix: "skilltool-staging"
      };
      const result = await runScenario(install.installDir, "skilltool-staging.mjs", cfg);
      assertSessionOk(result);
      expect(result.checks["stagedTokenPresent"], dump(result)).toBe(true);
    },
    11 * 60_000
  );

  it(
    "skills load returns the SKILL.md body into the model's context",
    async () => {
      const name = skillName("skilltool-body");
      const token = tok("SKILLBODY");
      const cfg: ScriptConfig = {
        skillName: name,
        files: [
          {
            path: "SKILL.md",
            content:
              `---\nname: ${name}\ndescription: Skill whose SKILL.md prose carries a body token to echo on load.\n---\n` +
              `# Body-delivery probe\nWhen you load this skill, reply with the body token below, exactly as written:\n\n` +
              `${token}\n\nDo not alter, translate, or explain it.`
          }
        ],
        system: "You are running an automated skill-body check. Rely only on the loaded skill's instructions.",
        message:
          `Call the skills tool for the skill named ${name} with action "load" to pull its instructions into context. Its body contains a ` +
          `token. Reply with that token exactly, and nothing else.`,
        // Token planted ONLY in the SKILL.md body ⇒ its presence proves the
        // skills load actually returned the body.
        checksExpr: `{ bodyTokenPresent: (haystackNorm.includes(${JSON.stringify(token)}) || assistantTextNorm.includes(${JSON.stringify(token)})) }`,
        idempotencyPrefix: "skilltool-body"
      };
      const result = await runScenario(install.installDir, "skilltool-body.mjs", cfg);
      assertSessionOk(result);
      expect(result.checks["bodyTokenPresent"], dump(result)).toBe(true);
    },
    11 * 60_000
  );

  it(
    "byte-caps a large SKILL.md at 400_000 bytes (near-start marker loads, beyond-cap marker is dropped)",
    async () => {
      const name = skillName("skilltool-bytecap");
      const startMarker = tok("CAPSTART");
      const beyondMarker = tok("CAPBEYOND");
      // Highly-repetitive filler: many BYTES (well past the 400_000-byte cap) but
      // few TOKENS (BPE collapses the sessions), so the capped body the model ingests
      // never blows the provider context window. ~120*3800 + newlines ≈ 459_800 B.
      const filler = ("x".repeat(120) + "\n").repeat(3800);
      const cfg: ScriptConfig = {
        skillName: name,
        files: [
          {
            path: "SKILL.md",
            content:
              `---\nname: ${name}\ndescription: Oversized skill exercising the 400000-byte SKILL.md load cap.\n---\n` +
              `# Oversized skill\n` +
              // Near the very start (byte ~150), well inside the 400_000-byte prefix.
              `${startMarker}\n` +
              `${filler}` +
              // Placed AFTER ~459_800 bytes of filler ⇒ beyond the 400_000-byte cap.
              `${beyondMarker}\n`
          }
        ],
        system: "You are running an automated byte-cap check. Rely only on the loaded skill's instructions.",
        message:
          `Call the skills tool for the skill named ${name} exactly once with action "load" to load its instructions. Near the top of ` +
          `those instructions is a passphrase line that starts with "CAPSTART-". Reply with only that passphrase.`,
        // Cap contract: the load handler returns utf8Prefix(body, 400000) plus a
        // "[skill SKILL.md truncated at 400000 bytes]" note. So the near-start
        // marker survives; the beyond-cap marker can never appear anywhere.
        checksExpr:
          `{ startMarkerPresent: (haystackNorm.includes(${JSON.stringify(startMarker)}) || assistantTextNorm.includes(${JSON.stringify(startMarker)})),` +
          ` beyondMarkerAbsent: !haystackNorm.includes(${JSON.stringify(beyondMarker)}),` +
          ` truncationNotePresent: (toolResultsJoined.includes("truncated at 400000 bytes") || toolResultsJoined.includes("skill SKILL.md truncated")) }`,
        idempotencyPrefix: "skilltool-bytecap"
      };
      const result = await runScenario(install.installDir, "skilltool-bytecap.mjs", cfg);
      assertSessionOk(result);
      // Early marker loaded (proves the skills load ran and the sub-cap prefix survived).
      expect(result.checks["startMarkerPresent"], dump(result)).toBe(true);
      // Core cap proof: the beyond-cap marker never reached the model or the stream.
      expect(result.checks["beyondMarkerAbsent"], dump(result)).toBe(true);
      expect(result.checks["truncationNotePresent"], dump(result)).toBe(true);
    },
    11 * 60_000
  );

  it(
    "redacts a secret-shaped value in the SKILL.md body (returned as [REDACTED], never verbatim)",
    async () => {
      const name = skillName("skilltool-redact");
      const guard = tok("GUARD");
      // Assembled (not a contiguous literal) so no real-looking key sits in source,
      // yet it still matches the runner's `sk-ant-[A-Za-z0-9_-]{16,}` shape.
      const secret = ["sk", "ant", "api03", "FAKE000FAKE111FAKE222ZZZ"].join("-");
      const cfg: ScriptConfig = {
        skillName: name,
        files: [
          {
            path: "SKILL.md",
            content:
              `---\nname: ${name}\ndescription: Skill whose body embeds a secret-shaped value to verify redaction.\n---\n` +
              `# Redaction probe\nGuard token (safe to echo): ${guard}\n\n` +
              `Example configuration (illustrative only, not a real credential):\nANTHROPIC_API_KEY=${secret}\n`
          }
        ],
        system: "You are running an automated redaction check. Rely only on the loaded skill's instructions.",
        message:
          `Call the skills tool for the skill named ${name} with action "load" to load its instructions. Near the top is a guard token ` +
          `beginning with "GUARD-". Reply with that guard token exactly.`,
        // guardTokenPresent proves the skills load ran and delivered the body;
        // secretLeaked scans the ENTIRE serialized session (every channel) for the raw
        // value; redactedMarkerPresent corroborates that redaction actually fired.
        checksExpr:
          `{ guardTokenPresent: (haystackNorm.includes(${JSON.stringify(guard)}) || assistantTextNorm.includes(${JSON.stringify(guard)})),` +
          ` secretLeaked: (haystack.includes(${JSON.stringify(secret)}) || haystackNorm.includes(${JSON.stringify(secret)})),` +
          ` redactedMarkerPresent: haystack.includes("[REDACTED]") }`,
        idempotencyPrefix: "skilltool-redact"
      };
      const result = await runScenario(install.installDir, "skilltool-redact.mjs", cfg);
      assertSessionOk(result);
      // The body reached the model (skills load ran).
      expect(result.checks["guardTokenPresent"], dump(result)).toBe(true);
      // Core redaction proof: the secret NEVER appears verbatim in any channel.
      expect(result.checks["secretLeaked"], dump(result)).toBe(false);
      // Positive corroboration that the redactor fired on the returned body.
      expect(result.checks["redactedMarkerPresent"], dump(result)).toBe(true);
    },
    11 * 60_000
  );
});
