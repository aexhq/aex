/**
 * [SECURITY/CORRECTNESS REGRESSION] High finding H9 — SDK docs ↔ code drift.
 *
 * Source: code review, 2026-05-23. Top-10 action #6.
 *
 * Issue
 * -----
 * The README and `packages/sdk/docs/*.md` have drifted before by advertising
 * removed or nonexistent helpers (`Skill.fromId(...)`, `uploadIfChanged(...)`,
 * two-arg `submit(...)`, etc.). The session redesign then DELETED `submit()` and
 * the entire run-id-addressed client surface (`getRun` / `stream` / `wait` /
 * `listRuns` / `readOutputText` / `download*` / `cancel` on the client) and
 * folded everything into sessions. The docs must teach the session surface and
 * must not resurrect the removed one.
 *
 * Locked invariant
 * ----------------
 * Every method the docs teach must exist on the shipped SDK surface, and no
 * published doc may advertise a removed method. This test is the single tripwire
 * that keeps the docs and the SDK aligned.
 */

import { describe, expect, it } from "vitest";
import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { resolve, dirname } from "node:path";
import {
  Aex,
  SessionClient,
  SessionHandle,
  AgentsMd,
  Sizes,
  File as AexFile
} from "../../src/index.js";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "..", "..", "..", "..");

function walkDocs(root: string): string[] {
  if (!existsSync(root)) return [];
  const entries: string[] = [];
  for (const name of readdirSync(root)) {
    const path = resolve(root, name);
    const stat = statSync(path);
    if (stat.isDirectory()) {
      entries.push(...walkDocs(path));
    } else if (/\.(md|tsx)$/.test(name)) {
      entries.push(path);
    }
  }
  return entries;
}

// The typedoc-generated SDK reference (`apps/docs/content/.../reference/sdk/`) is
// emitted verbatim from the JSDoc comments in `src/` on every `bun run generate`,
// so it mirrors the code itself. The composition-primitive JSDoc has been scrubbed
// of the removed `client.submit(...)` surface, so the generated mirror IS scanned
// too — that keeps the code comments honest, not just the hand-written guides.
const GENERATED_TYPEDOC = resolve(repoRoot, "apps", "docs", "content", "docs", "reference", "sdk");

function publishedDocFiles(): ReadonlyArray<{ readonly label: string; readonly content: string; readonly generated: boolean }> {
  const paths = [
    resolve(repoRoot, "README.md"),
    resolve(repoRoot, "packages", "sdk", "README.md"),
    ...walkDocs(resolve(repoRoot, "packages", "sdk", "docs")),
    ...walkDocs(resolve(repoRoot, "apps", "docs", "content")),
    ...walkDocs(resolve(repoRoot, "apps", "docs", "src", "content")),
    ...walkDocs(resolve(repoRoot, "apps", "docs", "components"))
  ];
  return paths.map((path) => ({
    label: path.replace(repoRoot, "").replace(/^[\\/]/, ""),
    content: readFileSync(path, "utf8"),
    generated: path.startsWith(GENERATED_TYPEDOC)
  }));
}

const isFn = (obj: object, key: string): boolean =>
  typeof (obj as unknown as Record<string, unknown>)[key] === "function";

describe("[REGRESSION] H9 — SDK docs ↔ code drift", () => {
  it("the session-first surface the docs teach exists on the shipped SDK", () => {
    const missing: string[] = [];

    // Client-level operations the docs call as `aex.<method>(...)`.
    for (const key of ["openSession", "run", "whoami", "deleteWorkspaceAsset"]) {
      if (!isFn(Aex.prototype, key)) missing.push(`Aex.prototype.${key}`);
    }
    // Workspace/session admin the docs call as `aex.sessions.<method>(...)`.
    // `outputs(id)` returns the SAME accessor as `session.outputs()` (shape
    // checked below), so id-addressed reads use `sessions.outputs(id).read(...)`.
    // Cross-run output search RELOCATED to `aex.outputs.search()` (WS6); the old
    // `sessions.searchOutputs` is gone.
    for (const key of ["create", "open", "get", "list", "outputs", "run"]) {
      if (!isFn(SessionClient.prototype, key)) missing.push(`SessionClient.prototype.${key}`);
    }
    // The lifecycle verbs a `session` handle keeps FLAT in the docs. The read /
    // stream / download surface was regrouped into accessor sub-resources
    // (`session.messages()/events()/outputs()/webhooks()`), checked below.
    for (const key of [
      "send",
      "refresh",
      "wait",
      "unit",
      "suspend",
      "cancel",
      "resume",
      "delete",
      "download",
      "downloadMetadata",
      "events",
      "outputs",
      "webhooks"
    ]) {
      if (!isFn(SessionHandle.prototype, key)) missing.push(`SessionHandle.prototype.${key}`);
    }

    // The docs now teach accessor groups — verify each accessor returns an
    // object exposing the verbs the guides chain onto it. Accessors build their
    // object literal synchronously (no I/O), so a bare handle over a stub HTTP
    // client is enough to assert the shape.
    const handle = new SessionHandle({} as never, { id: "ses_regression" } as never);
    const accessorVerbs: ReadonlyArray<{ readonly group: string; readonly verbs: readonly string[] }> = [
      { group: "messages", verbs: ["all", "list", "last", "first"] },
      { group: "events", verbs: ["list", "last", "first", "stream", "streamEnvelopes", "archiveLink", "download"] },
      { group: "outputs", verbs: ["list", "last", "first", "read", "find", "findOne", "search", "link", "fetch", "download"] },
      { group: "webhooks", verbs: ["list", "redeliver"] }
    ];
    for (const { group, verbs } of accessorVerbs) {
      const factory = (handle as unknown as Record<string, (() => object) | undefined>)[group];
      if (typeof factory !== "function") {
        missing.push(`session.${group}`);
        continue;
      }
      const accessor = factory.call(handle);
      for (const verb of verbs) {
        if (!isFn(accessor, verb)) missing.push(`session.${group}.${verb}`);
      }
    }

    expect(missing).toEqual([]);

    // Runtime sizing is exported as `Sizes` (the docs use `Sizes.SHARED_0_25X_1GB`);
    // the old `RuntimeSizes` export was removed.
    expect(typeof Sizes.SHARED_0_25X_1GB).toBe("string");
  });

  it("AgentsMd.fromPath and File.fromPath actually exist when referenced in public docs", () => {
    // Smoke check the AgentsMd / File static factories used by the public
    // composition docs. If they don't exist, every copy-paste throws TypeError.
    const hasStatic = (cls: object, key: string): boolean =>
      typeof (cls as unknown as Record<string, unknown>)[key] === "function";
    expect(hasStatic(AexFile, "fromPath")).toBe(true);
    expect(hasStatic(AgentsMd, "fromPath")).toBe(true);
    // Sanity: published docs still reference at least one of them (so this
    // file remains relevant).
    const docs = publishedDocFiles();
    expect(
      docs.some((doc) => /File\.fromPath\(/.test(doc.content) || /AgentsMd\.fromPath\(/.test(doc.content))
    ).toBe(true);
  });

  it("published docs do not advertise the removed submit / run-id-addressed client surface", () => {
    const forbidden: ReadonlyArray<{ readonly name: string; readonly needle: RegExp }> = [
      // Removed run-id-addressed client API — every read/control verb moved onto
      // the session handle or `aex.sessions.*`.
      { name: "client .submit(...)", needle: /\.submit\s*\(/ },
      { name: "client .getRun(...)", needle: /\.getRun\s*\(/ },
      { name: "client .getRunUnit(...)", needle: /\.getRunUnit\s*\(/ },
      { name: "client .getUnit(...)", needle: /\.getUnit\s*\(/ },
      { name: "client .listRuns(...)", needle: /\.listRuns\s*\(/ },
      { name: "client .readOutputText(...)", needle: /\.readOutputText\s*\(/ },
      { name: "client .waitForRun(...)", needle: /\.waitForRun\s*\(/ },
      // Removed `RuntimeSizes` export (use `Sizes`).
      { name: "RuntimeSizes export", needle: /\bRuntimeSizes\b/ },
      // Renamed data-source chat tools (run vocabulary -> session vocabulary).
      { name: "list_runs tool", needle: /\blist_runs\b/ },
      { name: "get_run tool", needle: /\bget_run\b/ },
      { name: "run_id tool arg", needle: /\brun_id\b/ },
      // Removed RunRef / ref-style run API.
      { name: "RunRef type", needle: /\bRunRef\b/ },
      { name: "ref.runId", needle: /\bref\.runId\b/ },
      { name: "runAndCollect alias", needle: /\brunAndCollect\b/ },
      { name: "secrets.get_value plaintext read", needle: /\.secrets\.get_value\s*\(/ },
      {
        name: "ref method",
        needle: /\bref\.(?:get|getUnit|events|stream|streamEnvelopes|wait|outputs|download|downloadOutput|downloadOutputs|downloadEvents|downloadMetadata|cancel|delete)\s*\(/
      },
      // The session read/stream/download surface moved from flat handle methods
      // onto accessor sub-resources (`session.events().list()`,
      // `session.outputs().read(...)`, `session.messages().last()`, …). The docs
      // must not resurrect the removed flat form invoked directly on a handle.
      // `session.download(...)` / `session.downloadMetadata(...)` stay flat, and
      // `session.events().streamEnvelopes(...)` (accessor form) is allowed —
      // only the direct `session.streamEnvelopes(...)` is forbidden.
      {
        name: "flat session-handle read/stream/download verb (now under messages()/events()/outputs()/webhooks())",
        needle:
          /\b(?:session|resumed|handle)\.(?:listEvents|streamEvents|streamEnvelopes|eventArchiveLink|downloadEvents|listOutputs|readOutput|findOutputs|findOutput|outputLink|fetchOutput|downloadOutputs|downloadOutput|webhookDeliveries|redeliverWebhook)\s*\(/
      }
    ];
    const failures: string[] = [];
    for (const doc of publishedDocFiles()) {
      for (const entry of forbidden) {
        if (entry.needle.test(doc.content)) {
          failures.push(`${doc.label} still advertises ${entry.name}`);
        }
      }
    }
    expect(failures).toEqual([]);
  });
});

/**
 * [REGRESSION] Pre-release fix-sweep (2026-07-05) — onboarding doc-drift guards.
 *
 * These pin the specific doc corrections the sweep made so they cannot silently
 * regress across CI: ONE canonical constructor form in onboarding prose, `npx
 * aex` in every local-install CLI snippet, `billing:read` minted in the
 * quickstart, no resurrected output "baseline snapshot" fiction, plus the typed
 * error / streaming-capability / custom-tool-entry / sandbox claims the shipped
 * code now guarantees.
 */
const readDoc = (rel: string): string =>
  readFileSync(resolve(repoRoot, rel), "utf8").replace(/\r\n/g, "\n");

// The hand-written onboarding surface a newcomer copy-pastes first.
const ONBOARDING_DOCS = [
  "README.md",
  "packages/sdk/README.md",
  "packages/sdk/docs/quickstart.md",
  "packages/sdk/docs/authentication.md",
  "packages/sdk/docs/billing.md"
] as const;

// The subset that walks a user from `npm i` straight into a CLI command.
const LOCAL_INSTALL_DOCS = [
  "README.md",
  "packages/sdk/README.md",
  "packages/sdk/docs/quickstart.md"
] as const;

// A bare `aex <verb>` command line (not prose like "aex is an agent…"): the
// footgun after a LOCAL `npm i`, where the binary is not on PATH.
const BARE_AEX_CMD =
  /^aex\s+(run|login|logout|whoami|auth|models|providers|tools|runtime-sizes|events|tail|inspect|wait|status|outputs|download|cancel|delete|delete-asset|billing|webhooks|sessions|runs|deliveries|debug)\b/m;

describe("[REGRESSION] pre-release fix-sweep — onboarding doc-drift", () => {
  it("onboarding docs use ONE constructor form: the string arg, never the object literal", () => {
    const failures: string[] = [];
    for (const rel of ONBOARDING_DOCS) {
      const content = readDoc(rel);
      // The object-literal form `new Aex({ ... })` is documented ONCE in the
      // credentials reference; onboarding prose must use `new Aex(apiKey)`.
      if (/new Aex\(\s*\{/.test(content)) failures.push(`${rel} shows the object-literal \`new Aex({ ... })\``);
      if (!/new Aex\(/.test(content)) failures.push(`${rel} no longer constructs \`new Aex(...)\` at all`);
    }
    expect(failures).toEqual([]);
  });

  it("local-install docs invoke the CLI as `npx aex …`, never a bare `aex <verb>`", () => {
    const failures: string[] = [];
    for (const rel of LOCAL_INSTALL_DOCS) {
      const content = readDoc(rel);
      // Sanity: these are the docs that teach `npm i` then run the CLI.
      if (!content.includes("npm i @aexhq/sdk")) failures.push(`${rel} no longer teaches \`npm i @aexhq/sdk\``);
      if (!content.includes("npx aex")) failures.push(`${rel} has no \`npx aex\` snippet`);
      if (BARE_AEX_CMD.test(content)) failures.push(`${rel} has a bare \`aex <verb>\` line (use \`npx aex\`)`);
    }
    expect(failures).toEqual([]);
  });

  it("the quickstart mints billing:read alongside the runs/outputs scopes", () => {
    const quickstart = readDoc("packages/sdk/docs/quickstart.md");
    for (const scope of ["runs:read", "runs:write", "outputs:read", "billing:read"]) {
      expect(quickstart).toContain(scope);
    }
  });

  it("outputs.md carries NO baseline-snapshot / filesystem-diff capture fiction", () => {
    const outputs = readDoc("packages/sdk/docs/outputs.md");
    const fictions = [
      /snapshots the filesystem/i,
      /filesystem diff/i,
      /uploads the delta/i,
      /baseline snapshot/i,
      /excluded by timing/i
    ];
    const hits = fictions.filter((f) => f.test(outputs)).map((f) => f.source);
    expect(hits).toEqual([]);
    // …and it still teaches the honest identity-based exclusion.
    expect(outputs).toMatch(/by IDENTITY/);
  });

  it("errors.md documents the typed hierarchy, idempotency conflict, and the empty-key throw", () => {
    const errors = readDoc("packages/sdk/docs/errors.md");
    for (const needle of [
      "apiCode",
      "idempotency_conflict",
      "AexIdempotencyConflictError",
      "isInsufficientScope",
      "isNotFound"
    ]) {
      expect(errors).toContain(needle);
    }
    // An empty idempotencyKey is a client-side fail-fast, not a wire round-trip.
    expect(errors).toMatch(/empty[^.]*idempotencyKey[^.]*throws|idempotencyKey[^.]*throws[^.]*RunConfigValidationError/i);
  });

  it("events.md documents capability-honest streaming (typed reject, not silent downgrade)", () => {
    const events = readDoc("packages/sdk/docs/events.md");
    expect(events).toContain("outputMode");
    expect(events).toMatch(/streamable/);
    expect(events).toMatch(/typed rejection|fail-closed|silent downgrade/i);
  });

  it("the custom-tool entry rule and sandbox notes are documented", () => {
    const tools = readDoc("packages/sdk/docs/concepts/agent-tools.md");
    expect(tools).toMatch(/\.mjs/);
    expect(tools).toMatch(/default-export/i);

    const limits = readDoc("packages/sdk/docs/limits-and-quotas.md");
    expect(limits).toContain("maxTurns");
    expect(limits).toMatch(/\/workspace/);
    expect(limits).toMatch(/PEP 668/);
  });
});
