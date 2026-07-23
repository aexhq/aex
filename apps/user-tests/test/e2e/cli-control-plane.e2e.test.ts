/**
 * LIVE BLACK-BOX end-to-end coverage for the aex CLI control-plane surface
 * (WS6: `login` / `auth status` / `orgs` / `workspaces` / `keys`).
 *
 * Black-box means: this file NEVER imports CLI internals. It spawns the built
 * `aex` bundle as a real subprocess with real args + env and asserts on
 * `exitCode` / `stdout` / `stderr` only. The only harness imports are the
 * shared child-process runner from `_fixtures/install.ts` (a generic spawn
 * utility, not CLI source).
 *
 * Auth bootstrap (fail-closed, no silent skip): a provisioned account PAT and
 * base URL come from env. If either is unset the module throws at import, which
 * fails the whole file — the intended gate for an on-demand live job.
 *
 *   AEX_TEST_ACCOUNT_TOKEN   account PAT (`aexu_...`) — the control-plane bearer.
 *   AEX_API_URL              absolute http(s) hosted API base URL.
 *   AEX_E2E_CLI_ENTRY        optional: path to the built CLI entry to drive.
 *                            Defaults to `packages/cli/dist/cli.mjs`.
 *
 * Config isolation: every spawned command points `XDG_CONFIG_HOME` AND `APPDATA`
 * at a fresh per-test tempdir, so the real `~/.config/aex/config.json` is never
 * read or written.
 *
 * EXPECTED FAILURE MODE: against a plane where the WS6 control-plane endpoints
 * are not deployed, the live happy-path flows (org/workspace/key create/list,
 * invite, and login-via-whoami) FAIL-CLOSED — the CLI reaches the endpoint and
 * gets a control-plane error, so `expect(exitCode).toBe(0)` fails. That is the
 * captured proof the suite is correctly wired; it is NOT faked into a pass. The
 * deterministic wiring/redaction proofs (bad-token-not-persisted, last-4
 * redaction, keys mutual-exclusion, PAT non-leak) pass independently of any
 * deployment, proving the black-box harness drives the binary correctly.
 */
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { getBunCommand, runCommand, type SessionResult } from "../_fixtures/install.js";

function requireEnv(name: string): string {
  const value = process.env[name];
  if (value === undefined || value.length === 0) {
    throw new Error(
      `cli-control-plane e2e: required env ${name} is missing — this suite is fail-closed and runs only in the gated live job`
    );
  }
  return value;
}

const accountToken = requireEnv("AEX_TEST_ACCOUNT_TOKEN");
const apiBase = requireEnv("AEX_API_URL").replace(/\/+$/, "");
if (!/^https?:\/\//.test(apiBase)) {
  throw new Error(`cli-control-plane e2e: AEX_API_URL must be an absolute http(s) URL (got: ${apiBase})`);
}

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "..", "..", "..", "..");
const cliEntry = process.env.AEX_E2E_CLI_ENTRY ?? join(repoRoot, "packages", "cli", "dist", "cli.mjs");

/** Config dirs created during the run; removed in afterAll. */
const configDirs: string[] = [];
/** Control-plane resources created against a LIVE plane; best-effort cleanup. */
const createdWorkspaceIds: string[] = [];
const createdKeyIds: string[] = [];

function freshConfigDir(): string {
  const dir = mkdtempSync(join(tmpdir(), "aex-e2e-cfg-"));
  configDirs.push(dir);
  return dir;
}

function redact(text: string): string {
  return text.split(accountToken).join("[REDACTED_ACCOUNT_TOKEN]");
}

function diag(label: string, result: SessionResult): string {
  return redact(
    `${label} exited ${result.exitCode}\n--- stdout ---\n${result.stdout}\n--- stderr ---\n${result.stderr}`
  );
}

/**
 * Assert a secret does not appear in `haystack` WITHOUT ever passing the secret
 * to a matcher that would print it on failure. The boolean is what the matcher
 * renders, so a leak surfaces as `true !== false` plus a redacted message.
 */
function expectAbsent(haystack: string, secret: string, message: string): void {
  expect(haystack.includes(secret), message).toBe(false);
}

function countOccurrences(haystack: string, needle: string): number {
  if (needle.length === 0) return 0;
  return haystack.split(needle).length - 1;
}

/**
 * Drive the built CLI as a subprocess in an isolated config dir. Flow 6 (secret
 * hygiene) is enforced on EVERY invocation: the account PAT is a bearer
 * credential and must never appear on any output channel.
 */
async function runCli(args: readonly string[], configDir: string, timeoutMs = 60_000): Promise<SessionResult> {
  const result = await runCommand(getBunCommand(), [cliEntry, ...args], {
    cwd: repoRoot,
    timeoutMs,
    env: { XDG_CONFIG_HOME: configDir, APPDATA: configDir }
  });
  const combined = `${result.stdout}\n${result.stderr}`;
  expectAbsent(combined, accountToken, `account PAT leaked in output of: aex ${redact(args.join(" "))}`);
  return result;
}

/** Common auth + target flags for a control-plane verb driven by the account PAT. */
function auth(...args: string[]): string[] {
  return [...args, "--api-key", accountToken, "--aex-url", apiBase];
}

/** Create an org and return its id. Fails-closed (throws) if the create did not succeed. */
async function createOrg(configDir: string, name: string): Promise<string> {
  const create = await runCli(auth("orgs", "create", "--name", name), configDir);
  expect(create.exitCode, diag("aex orgs create", create)).toBe(0);
  const org = JSON.parse(create.stdout.trim()) as { id?: unknown };
  expect(typeof org.id, diag("aex orgs create", create)).toBe("string");
  return org.id as string;
}

/** Best-effort delete of a control-plane resource created during a live run (no assertions). */
async function bestEffortDelete(kind: "workspaces" | "keys", id: string): Promise<void> {
  try {
    await runCommand(getBunCommand(), [cliEntry, ...auth(kind, "delete", id)], {
      cwd: repoRoot,
      timeoutMs: 30_000,
      env: { XDG_CONFIG_HOME: freshConfigDir(), APPDATA: freshConfigDir() }
    });
  } catch {
    // Cleanup is best-effort; the plane may be undeployed or the id never real.
  }
}

beforeAll(() => {
  if (!existsSync(cliEntry)) {
    throw new Error(
      `cli-control-plane e2e: built CLI entry not found at ${cliEntry}\n` +
        "Build it first: bun run --filter @aexhq/cli build (or set AEX_E2E_CLI_ENTRY)."
    );
  }
  // Non-secret target header for the run log.
  process.stderr.write(`[cli-control-plane e2e] driving ${cliEntry} against ${apiBase}\n`);
});

afterAll(async () => {
  await Promise.all(createdKeyIds.map((id) => bestEffortDelete("keys", id)));
  await Promise.all(createdWorkspaceIds.map((id) => bestEffortDelete("workspaces", id)));
  for (const dir of configDirs) {
    rmSync(dir, { recursive: true, force: true });
  }
});

describe("flow 1 — login + auth status", () => {
  // LIVE (fails-closed until the control-plane whoami is deployed and accepts the
  // PAT). The CLI now detects the `aexu_` PAT and validates it against the
  // CONTROL-plane (dashboard-BFF) whoami — persisting `accountToken`, not the
  // data-plane `apiKey` — so this no longer 400s (`malformed_token`) client-side
  // against a live plane; it exercises the real control-plane whoami path.
  it("login --api-key <account PAT> persists a credential; auth status reveals only the last 4", async () => {
    const cfg = freshConfigDir();
    const login = await runCli(["login", "--api-key", accountToken, "--aex-url", apiBase], cfg);
    expect(login.exitCode, diag("aex login --api-key", login)).toBe(0);

    const status = await runCli(["auth", "status"], cfg);
    expect(status.exitCode, diag("aex auth status", status)).toBe(0);
    const doc = JSON.parse(status.stdout.trim()) as { hasToken?: unknown; tokenSuffix?: unknown };
    expect(doc.hasToken, diag("aex auth status", status)).toBe(true);
    expect(doc.tokenSuffix, diag("aex auth status", status)).toBe(accountToken.slice(-4));
    expectAbsent(status.stdout, accountToken, "auth status must not print the full token");
  });

  // DETERMINISTIC (passes without any deployment): a bad token is never persisted.
  it("login with a bad token exits non-zero and persists nothing", async () => {
    const cfg = freshConfigDir();
    const badToken = "aexu_bad_token_definitely_invalid_0000";
    const login = await runCli(["login", "--api-key", badToken, "--aex-url", apiBase], cfg);
    expect(login.exitCode, diag("aex login (bad token)", login)).not.toBe(0);
    expect(existsSync(join(cfg, "aex", "config.json")), "a failed login must not write a config file").toBe(false);

    const status = await runCli(["auth", "status"], cfg);
    const doc = JSON.parse(status.stdout.trim()) as { hasToken?: unknown };
    expect(doc.hasToken, diag("aex auth status after bad login", status)).toBe(false);
  });

  // DETERMINISTIC: auth status fingerprints a stored credential as last-4 only.
  it("auth status prints only the last 4 of a stored credential, never the full secret", async () => {
    const cfg = freshConfigDir();
    mkdirSync(join(cfg, "aex"), { recursive: true });
    // Synthetic seed — deliberately NOT the real PAT — written the way the CLI would.
    const seeded = "aexu_seeded_control_plane_secret_TAIL";
    writeFileSync(
      join(cfg, "aex", "config.json"),
      JSON.stringify({ schemaVersion: 1, apiKey: seeded, aexUrl: apiBase }) + "\n",
      { mode: 0o600 }
    );

    const status = await runCli(["auth", "status"], cfg);
    expect(status.exitCode, diag("aex auth status (seeded)", status)).toBe(0);
    const doc = JSON.parse(status.stdout.trim()) as { hasToken?: unknown; tokenSuffix?: unknown };
    expect(doc.hasToken, diag("aex auth status (seeded)", status)).toBe(true);
    expect(doc.tokenSuffix, diag("aex auth status (seeded)", status)).toBe("TAIL");
    expectAbsent(status.stdout, seeded, "auth status leaked the full seeded secret");
    expectAbsent(status.stdout, "control_plane_secret", "auth status leaked a secret substring");
  });
});

describe("flow 2 — orgs", () => {
  // LIVE (fails-closed until /api/orgs is deployed).
  it("orgs create prints an org id; list contains it; members shows the caller as admin", async () => {
    const cfg = freshConfigDir();
    const orgId = await createOrg(cfg, `e2e ${new Date().toISOString()}`);

    const list = await runCli(auth("orgs", "list"), cfg);
    expect(list.exitCode, diag("aex orgs list", list)).toBe(0);
    const orgs = JSON.parse(list.stdout.trim()) as Array<{ id?: unknown }>;
    expect(orgs.map((o) => o.id), diag("aex orgs list", list)).toContain(orgId);

    const members = await runCli(auth("orgs", "members", orgId), cfg);
    expect(members.exitCode, diag("aex orgs members", members)).toBe(0);
    const rows = JSON.parse(members.stdout.trim()) as Array<{ role?: unknown }>;
    expect(
      rows.some((m) => m.role === "admin"),
      diag("aex orgs members", members)
    ).toBe(true);
  });
});

describe("flow 3 — workspaces", () => {
  // LIVE (fails-closed until /api/workspaces is deployed).
  it("workspaces create reveals the one-time key once; list contains it; delete succeeds", async () => {
    const cfg = freshConfigDir();
    const orgId = await createOrg(cfg, `e2e-ws ${new Date().toISOString()}`);

    const create = await runCli(auth("workspaces", "create", "--org", orgId, "--name", "ws"), cfg);
    expect(create.exitCode, diag("aex workspaces create", create)).toBe(0);
    const ws = JSON.parse(create.stdout.trim()) as { workspaceId?: unknown; apiKey?: unknown };
    expect(typeof ws.workspaceId, diag("aex workspaces create", create)).toBe("string");
    expect(typeof ws.apiKey, diag("aex workspaces create", create)).toBe("string");
    const workspaceId = ws.workspaceId as string;
    const oneTimeKey = ws.apiKey as string;
    createdWorkspaceIds.push(workspaceId);
    // The one-time key is printed exactly ONCE, on stdout, and never on stderr.
    expect(countOccurrences(create.stdout, oneTimeKey), "one-time key must be printed exactly once").toBe(1);
    expectAbsent(create.stderr, oneTimeKey, "one-time workspace key must not appear on stderr");

    const list = await runCli(auth("workspaces", "list"), cfg);
    expect(list.exitCode, diag("aex workspaces list", list)).toBe(0);
    const rows = JSON.parse(list.stdout.trim()) as Array<{ id?: unknown }>;
    expect(rows.map((r) => r.id), diag("aex workspaces list", list)).toContain(workspaceId);
    // The plaintext workspace key is a one-time reveal — the list must not echo it.
    expectAbsent(list.stdout, oneTimeKey, "workspaces list leaked the one-time key");

    const del = await runCli(auth("workspaces", "delete", workspaceId), cfg);
    expect(del.exitCode, diag("aex workspaces delete", del)).toBe(0);
  });
});

describe("flow 4 — keys", () => {
  // LIVE (fails-closed until /api/keys is deployed).
  it("keys create <wsId> reveals a one-time key; list hides the plaintext; --account mints a PAT; delete succeeds", async () => {
    const cfg = freshConfigDir();
    const orgId = await createOrg(cfg, `e2e-keys ${new Date().toISOString()}`);
    const wsCreate = await runCli(auth("workspaces", "create", "--org", orgId, "--name", "ws"), cfg);
    expect(wsCreate.exitCode, diag("aex workspaces create", wsCreate)).toBe(0);
    const ws = JSON.parse(wsCreate.stdout.trim()) as { workspaceId?: unknown };
    expect(typeof ws.workspaceId, diag("aex workspaces create", wsCreate)).toBe("string");
    const workspaceId = ws.workspaceId as string;
    createdWorkspaceIds.push(workspaceId);

    const create = await runCli(auth("keys", "create", workspaceId), cfg);
    expect(create.exitCode, diag("aex keys create <wsId>", create)).toBe(0);
    const key = JSON.parse(create.stdout.trim()) as { id?: unknown; apiKey?: unknown };
    expect(typeof key.id, diag("aex keys create <wsId>", create)).toBe("string");
    expect(typeof key.apiKey, diag("aex keys create <wsId>", create)).toBe("string");
    const keyId = key.id as string;
    const oneTimeKey = key.apiKey as string;
    createdKeyIds.push(keyId);

    const list = await runCli(auth("keys", "list"), cfg);
    expect(list.exitCode, diag("aex keys list", list)).toBe(0);
    // Metadata only — the plaintext value must never come back from a list.
    expectAbsent(list.stdout, oneTimeKey, "keys list leaked the plaintext key value");
    const rows = JSON.parse(list.stdout.trim()) as Array<Record<string, unknown>>;
    expect(
      rows.every((r) => !("apiKey" in r)),
      diag("aex keys list", list)
    ).toBe(true);

    const pat = await runCli(auth("keys", "create", "--account"), cfg);
    expect(pat.exitCode, diag("aex keys create --account", pat)).toBe(0);
    const patKey = JSON.parse(pat.stdout.trim()) as { id?: unknown; apiKey?: unknown };
    expect(typeof patKey.apiKey, diag("aex keys create --account", pat)).toBe("string");
    if (typeof patKey.id === "string") createdKeyIds.push(patKey.id);

    const del = await runCli(auth("keys", "delete", keyId), cfg);
    expect(del.exitCode, diag("aex keys delete", del)).toBe(0);
  });

  // DETERMINISTIC: mutual exclusion is enforced in arg-parsing, before any network call.
  it("keys create <wsId> --account is rejected (mutual exclusion)", async () => {
    const cfg = freshConfigDir();
    const result = await runCli(auth("keys", "create", "wsp_example", "--account"), cfg);
    expect(result.exitCode, diag("aex keys create <wsId> --account", result)).toBe(2);
    expect(result.stderr, diag("aex keys create <wsId> --account", result)).toContain("not both");
  });
});

describe("flow 5 — org invite", () => {
  // LIVE (fails-closed until /api/orgs/:id/invites is deployed).
  it("orgs invite <id> --email --role member succeeds", async () => {
    const cfg = freshConfigDir();
    const orgId = await createOrg(cfg, `e2e-invite ${new Date().toISOString()}`);
    const invite = await runCli(
      auth("orgs", "invite", orgId, "--email", "e2e-invitee@aexhq.test", "--role", "member"),
      cfg
    );
    expect(invite.exitCode, diag("aex orgs invite", invite)).toBe(0);
    const doc = JSON.parse(invite.stdout.trim()) as { email?: unknown; role?: unknown };
    expect(doc.email, diag("aex orgs invite", invite)).toBe("e2e-invitee@aexhq.test");
    expect(doc.role, diag("aex orgs invite", invite)).toBe("member");
  });
});

describe("flow 6 — secret hygiene", () => {
  // Runs a representative spread of verbs and confirms the account PAT never
  // surfaces. `runCli` already asserts non-leak on every call; this documents
  // the invariant as a first-class test.
  it("no command echoes the account PAT on stdout or stderr", async () => {
    const cfg = freshConfigDir();
    const status = await runCli(["auth", "status"], cfg);
    const orgs = await runCli(auth("orgs", "list"), cfg);
    const keys = await runCli(auth("keys", "list"), cfg);
    expectAbsent(`${status.stdout}${status.stderr}`, accountToken, "auth status leaked the account PAT");
    expectAbsent(`${orgs.stdout}${orgs.stderr}`, accountToken, "orgs list leaked the account PAT");
    expectAbsent(`${keys.stdout}${keys.stderr}`, accountToken, "keys list leaked the account PAT");
  });
});
