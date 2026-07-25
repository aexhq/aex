/**
 * Edge-case sweep for the installed `aex` CLI binary — arg parsing, usage,
 * auth resolution, and error paths that do NOT require the live API.
 *
 * Blackbox: installs the packed/published SDK artifact into a clean tempdir and
 * spawns the shipped `aex` bin. Complements `cli-bin.test.ts` /
 * `cli-host-commands.test.ts` by probing the failure/usage surface a real user
 * hits on day one (typos, missing flags, no token, removed flags, secrets).
 *
 * Every case here is hermetic: it either short-circuits before any network call
 * or is intentionally pointed at an unroutable localhost port. Persistent
 * config is isolated via XDG_CONFIG_HOME so a developer's real `aex login`
 * cannot change the outcome of the "no token" cases.
 */
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { getAexBinPath, installAex, runCommand, type InstallResult, type SessionResult } from "../_fixtures/install.js";

describe("installed aex CLI — offline edge cases", () => {
  let install: InstallResult;
  let binPath: string;
  let emptyConfigDir: string;

  beforeAll(async () => {
    install = await installAex();
    binPath = getAexBinPath(install.installDir);
    // Point the persistent config store at a guaranteed-empty dir so no
    // developer `aex login` leaks a stored token into the "no token" cases.
    emptyConfigDir = mkdtempSync(join(tmpdir(), "aex-edge-cli-cfg-"));
  }, 240_000);

  afterAll(() => {
    if (emptyConfigDir) rmSync(emptyConfigDir, { recursive: true, force: true });
    install?.cleanup();
  });

  async function executeCli(args: readonly string[], timeoutMs = 30_000): Promise<SessionResult> {
    return await runCommand(binPath, args, {
      cwd: install.installDir,
      timeoutMs,
      env: { XDG_CONFIG_HOME: emptyConfigDir }
    });
  }

  function diag(label: string, r: SessionResult): string {
    return `${label} exited ${r.exitCode}\n--- stdout ---\n${r.stdout}\n--- stderr ---\n${r.stderr}`;
  }

  // ---------------------------------------------------------------- help / version

  it("no args prints usage and exits 0", async () => {
    const r = await executeCli([]);
    expect(r.exitCode, diag("aex (no args)", r)).toBe(0);
    expect(r.stdout).toMatch(/Usage:/);
    expect(r.stdout).toMatch(/aex start/);
  });

  it("--help exits 0 with usage banner", async () => {
    const r = await executeCli(["--help"]);
    expect(r.exitCode, diag("aex --help", r)).toBe(0);
    expect(r.stdout).toMatch(/Usage:/);
    expect(r.stdout).toMatch(/aex whoami/);
  });

  it("--version has no dedicated handler (falls through to unknown-subcommand)", async () => {
    const r = await executeCli(["--version"]);
    // Documents the current behavior: there is NO --version/-v affordance; it
    // degrades to the generic unknown-subcommand path (clean, non-zero, points
    // at --help) rather than printing the installed version or crashing.
    expect(r.exitCode, diag("aex --version", r)).toBe(2);
    expect(r.stderr).toMatch(/unknown subcommand: --version/);
    expect(r.stderr).not.toMatch(/internal_error/);
    // never a raw stack trace
    expect(r.stderr).not.toMatch(/\bat .+\(.+:\d+:\d+\)/);
  });

  it("-v has no dedicated handler either", async () => {
    const r = await executeCli(["-v"]);
    expect(r.exitCode, diag("aex -v", r)).toBe(2);
    expect(r.stderr).toMatch(/unknown subcommand: -v/);
  });

  // ---------------------------------------------------------------- unknown routing

  it("unknown subcommand exits 2 and points at --help", async () => {
    const r = await executeCli(["frobnicate"]);
    expect(r.exitCode, diag("aex frobnicate", r)).toBe(2);
    expect(r.stderr).toMatch(/unknown subcommand: frobnicate/);
    expect(r.stderr).toMatch(/aex --help/);
  });

  // ---------------------------------------------------------------- start: usage errors

  it("start rejects an unknown flag with exit 2", async () => {
    const r = await executeCli([
      "start",
      "--model", "anthropic/claude-haiku-4-5",
      "--prompt", "hi",
      "--api-key", "dummy",
      "--totally-bogus"
    ]);
    expect(r.exitCode, diag("aex start --totally-bogus", r)).toBe(2);
    expect(r.stderr).toBe("aex start --totally-bogus: unknown flag\n");
  });

  it("start needs NO provider key — the removed per-provider flags are rejected", async () => {
    // Under managed model access there is no customer key to supply, so the
    // --<provider>-api-key flags are gone. A stale invocation must fail loudly
    // rather than be quietly accepted and ignored.
    const r = await executeCli([
      "start", "--anthropic-api-key", "k",
      "--model", "anthropic/claude-haiku-4-5", "--prompt", "hi", "--api-key", "dummy"
    ]);
    expect(r.exitCode, diag("aex start --anthropic-api-key", r)).toBe(2);
    expect(r.stderr).toMatch(/unknown flag/);
  });

  it("session without --model exits 2", async () => {
    const r = await executeCli(["start", "--prompt", "hi", "--api-key", "dummy"]);
    expect(r.exitCode, diag("aex start (no model)", r)).toBe(2);
    expect(r.stderr).toBe("aex start --model: is required when --config is not provided\n");
  });

  it("session without --prompt exits 2", async () => {
    const r = await executeCli(["start", "--model", "anthropic/claude-haiku-4-5", "--api-key", "dummy"]);
    expect(r.exitCode, diag("aex start (no prompt)", r)).toBe(2);
    expect(r.stderr).toBe("aex start --prompt: is required (repeatable)\n");
  });

  it("a bare model name is a USAGE error naming the slug shape (exit 2)", async () => {
    const r = await executeCli([
      "start",
      "--model", "claude-haiku",
      "--prompt", "hi",
      "--api-key", "dummy"
    ]);
    expect(r.exitCode, diag("aex start bad model", r)).toBe(2);
    // A bare name is no longer a model id: the gateway takes creator/model slugs.
    // This is the single most common migration mistake, so it must read as a flag
    // error (exit 2, plain line) and not as a failed session (exit 1, JSON envelope).
    expect(r.stderr).toMatch(/^aex start --model: --model must be a gateway model slug/);
    expect(r.stderr).toMatch(/got "claude-haiku"/);
    expect(r.stderr).not.toMatch(/session_failed/);
  });

  it("the removed --provider flag exits 2 without leaking SDK internals", async () => {
    // There is no provider to select: it is the model slug's creator prefix.
    const r = await executeCli([
      "start",
      "--provider", "anthropic",
      "--model", "anthropic/claude-haiku-4-5",
      "--prompt", "hi",
      "--api-key", "dummy"
    ]);
    expect(r.exitCode, diag("aex start --provider", r)).toBe(2);
    expect(r.stderr).toMatch(/unknown flag/);
    expect(r.stderr).not.toMatch(/Aex\.start|Skill\.fromContent|Tool\.fromFiles/);
  });

  it("removed --proxy-endpoint flag on start exits 2 with a migration hint", async () => {
    const r = await executeCli([
      "start",
      "--model", "anthropic/claude-haiku-4-5",
      "--prompt", "hi",
      "--proxy-endpoint", "https://example.com",
      "--api-key", "dummy"
    ]);
    expect(r.exitCode, diag("aex start --proxy-endpoint", r)).toBe(2);
    expect(r.stderr).toMatch(/no longer supported/);
  });

  // ---------------------------------------------------------------- auth / flag-value

  it("host verb without a token (isolated config) exits 2 pointing at aex login", async () => {
    const r = await executeCli(["status", "some-session-id"]);
    expect(r.exitCode, diag("aex status (no token)", r)).toBe(2);
    expect(r.stderr).toMatch(/no API key/);
    expect(r.stderr).toMatch(/aex login/);
  });

  it("host verb with a token but no id exits 2 with usage", async () => {
    const r = await executeCli(["status", "--api-key", "dummy"]);
    expect(r.exitCode, diag("aex status (no id)", r)).toBe(2);
    expect(r.stderr).toMatch(/usage: aex status <session-id>/);
  });

  it("--api-key with no value exits 2", async () => {
    const r = await executeCli(["status", "some-session-id", "--api-key"]);
    expect(r.exitCode, diag("aex status --api-key (no value)", r)).toBe(2);
    expect(r.stderr).toMatch(/--api-key requires a value/);
  });

  it("removed --workspace flag exits 2 with a migration hint", async () => {
    const r = await executeCli(["status", "some-session-id", "--workspace", "ws-1", "--api-key", "dummy"]);
    expect(r.exitCode, diag("aex status --workspace", r)).toBe(2);
    expect(r.stderr).toMatch(/workspace is derived from --api-key/);
  });

  it("a malformed --timeout duration exits 2 before any network call", async () => {
    const r = await executeCli(["wait", "some-session-id", "--timeout", "banana", "--api-key", "dummy"]);
    expect(r.exitCode, diag("aex wait --timeout banana", r)).toBe(2);
    expect(r.stderr).toMatch(/--timeout: invalid duration/);
  });

  it("a negative --timeout duration is rejected (exit 2)", async () => {
    const r = await executeCli(["wait", "some-session-id", "--timeout", "-5s", "--api-key", "dummy"]);
    expect(r.exitCode, diag("aex wait --timeout -5s", r)).toBe(2);
    expect(r.stderr).toMatch(/--timeout: invalid duration/);
  });

  // ---------------------------------------------------------------- discovery reads (no token/network)

  it("the removed models/providers verbs exit 2 (there is no closed catalog to list)", async () => {
    // The managed gateway arbitrates an OPEN slug space, so aex no longer ships a
    // model or provider catalog to enumerate. Both verbs are gone; a stale script
    // calling them must fail, not silently print nothing and exit 0.
    for (const verb of ["models", "providers"] as const) {
      const r = await executeCli([verb, "list"]);
      expect(r.exitCode, diag(`aex ${verb} list`, r)).toBe(2);
      expect(r.stderr).toMatch(/unknown subcommand/);
    }
  });

  it("all discovery verbs accept installed --json in every optional-list position", async () => {
    for (const verb of ["tools", "runtime-sizes"] as const) {
      for (const tail of [
        ["--json"],
        ["--json", "list"],
        ["list", "--json"],
        ["--json", "list", "--json"]
      ] as const) {
        const r = await executeCli([verb, ...tail]);
        expect(r.exitCode, diag(`aex ${verb} ${tail.join(" ")}`, r)).toBe(0);
        expect(Array.isArray(JSON.parse(r.stdout.trim()))).toBe(true);
        expect(r.stderr).toBe("");
      }
    }
  });

  it("the installed global parser leaves near-prefix, equals-like, and -- tokens command-owned", async () => {
    for (const arg of ["--jsonish", "--json=true", "--"] as const) {
      const r = await executeCli(["tools", arg, "--json"]);
      expect(r.exitCode, diag(`aex tools ${arg} --json`, r)).toBe(2);
      expect(r.stdout).toBe("");
      expect(r.stderr).toBe(`unknown flag: ${arg}\nusage: aex tools list [--json]\n`);
    }
  });

  it("a discovery verb with a stray positional arg exits 2", async () => {
    const r = await executeCli(["tools", "list", "garbage-arg"]);
    expect(r.exitCode, diag("aex tools list garbage-arg", r)).toBe(2);
    expect(r.stderr).toMatch(/unexpected arguments: garbage-arg/);
  });

  // ---------------------------------------------------------------- secret hygiene

  it("--debug never echoes the api key or provider key, even on an error path", async () => {
    const SECRET_TOKEN = "SUPERSECRETTOKEN-do-not-leak-4711";
    const r = await executeCli([
      "start",
      "--model", "definitely-not-a-model",
      "--prompt", "hi",
      "--api-key", SECRET_TOKEN,
      "--debug"
    ]);
    // Rejected at model validation (no network), but --debug has already
    // printed the auth-source line to stderr by then.
    expect(r.exitCode, diag("aex start --debug (bad model)", r)).toBe(2);
    expect(r.stderr).toMatch(/definitely-not-a-model/);
    const combined = r.stdout + r.stderr;
    expect(combined, "api key leaked to output").not.toContain(SECRET_TOKEN);
    // the debug line should confirm the source without the value
    expect(r.stderr).toMatch(/\[aex\] auth: --api-key flag/);
  });

  // ---------------------------------------------------------------- unknown-flag consistency (FINDING)

  it("whoami and read verbs both reject unknown flags before network", async () => {
    // whoami: extra args (incl. unknown --flags) are rejected -> exit 2.
    const whoami = await executeCli(["whoami", "--typo-flag", "--api-key", "dummy"]);
    expect(whoami.exitCode, diag("aex whoami --typo-flag", whoami)).toBe(2);
    expect(whoami.stderr).toMatch(/unexpected arguments: --typo-flag/);

    const status = await executeCli([
      "status", "some-session-id",
      "--typo-flag",
      "--api-key", "dummy",
      "--aex-url", "http://127.0.0.1:9"
    ]);
    expect(status.exitCode, diag("aex status --typo-flag", status)).toBe(2);
    expect(status.stderr).toMatch(/unknown flag: --typo-flag/);
    expect(status.stderr).toMatch(/usage: aex status/);
    expect(status.stderr).not.toMatch(/status_failed/);
  });
});
