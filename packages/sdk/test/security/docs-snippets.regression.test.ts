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
 * the entire session-id-addressed client surface (`getSessionRecord` / `stream` / `wait` /
 * `listSessionRecords` / `readSessionFileText` / `download*` / `cancel` on the client) and
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
  DEFAULT_RUNTIME_KIND,
  Instructions,
  RUNTIME_KINDS,
  Sizes,
  File as AexFile
} from "../../src/index.js";
import { SessionClient, SessionHandle } from "../../src/client.js";

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
    for (const key of ["start", "whoami"]) {
      if (!isFn(Aex.prototype, key)) missing.push(`Aex.prototype.${key}`);
    }
    // Workspace/session admin the docs call as `aex.sessions.<method>(...)`.
    for (const key of ["create", "open", "get", "list", "delete"]) {
      if (!isFn(SessionClient.prototype, key)) missing.push(`SessionClient.prototype.${key}`);
    }
    // The lifecycle verbs a `session` handle keeps FLAT in the docs. The read /
    // stream / download surface was regrouped into accessor sub-resources
    // (`session.messages/events/files/webhooks`), checked below.
    for (const key of [
      "refresh",
      "suspend",
      "cancel",
      "resume",
      "delete",
      "download",
      "downloadMetadata"
    ]) {
      if (!isFn(SessionHandle.prototype, key)) missing.push(`SessionHandle.prototype.${key}`);
    }

    // The docs now teach accessor groups — verify each accessor returns an
    // object exposing the verbs the guides chain onto it. Accessors build their
    // object literal synchronously (no I/O), so a bare handle over a stub HTTP
    // client is enough to assert the shape.
    const handle = new SessionHandle({} as never, { id: "ses_regression" } as never);
    const accessorVerbs: ReadonlyArray<{ readonly group: string; readonly verbs: readonly string[] }> = [
      { group: "messages", verbs: ["send", "replayLast", "list", "last", "first"] },
      { group: "events", verbs: ["iterate", "list", "last", "first", "stream", "streamEnvelopes", "archiveLink", "download"] },
      { group: "files", verbs: ["list", "last", "first", "read", "find", "findOne", "link", "fetch", "download"] },
      { group: "webhooks", verbs: ["list", "redeliver"] }
    ];
    for (const { group, verbs } of accessorVerbs) {
      const accessor = (handle as unknown as Record<string, object | undefined>)[group];
      if (accessor === undefined) {
        missing.push(`session.${group}`);
        continue;
      }
      for (const verb of verbs) {
        if (!isFn(accessor, verb)) missing.push(`session.${group}.${verb}`);
      }
    }

    expect(missing).toEqual([]);

    // Runtime sizing is exported as `Sizes` (the docs use `Sizes.CPU_0_25_1GB`);
    // the old `RuntimeSizes` export was removed.
    expect(typeof Sizes.CPU_0_25_1GB).toBe("string");
  });

  it("Instructions.fromPath and File.fromPath exist when referenced in public docs", () => {
    // Smoke check the Instructions / File static factories used by public
    // composition docs. If they don't exist, every copy-paste throws TypeError.
    const hasStatic = (cls: object, key: string): boolean =>
      typeof (cls as unknown as Record<string, unknown>)[key] === "function";
    expect(hasStatic(AexFile, "fromPath")).toBe(true);
    expect(hasStatic(Instructions, "fromPath")).toBe(true);
    // Sanity: published docs still reference at least one of them (so this
    // file remains relevant).
    const docs = publishedDocFiles();
    expect(
      docs.some((doc) => /File\.fromPath\(/.test(doc.content) || /Instructions\.fromPath\(/.test(doc.content))
    ).toBe(true);
  });

  it("published docs do not advertise the removed submit / session-id-addressed client surface", () => {
    const forbidden: ReadonlyArray<{ readonly name: string; readonly needle: RegExp }> = [
      // Removed session-id-addressed client API — every read/control verb moved onto
      // the session handle or `aex.sessions.*`.
      { name: "client .submit(...)", needle: /\.submit\s*\(/ },
      { name: "client .getSessionRecord(...)", needle: /\.getSessionRecord\s*\(/ },
      { name: "client .getSessionUnit(...)", needle: /\.getSessionUnit\s*\(/ },
      { name: "manufactured session.unit() aggregate", needle: /\bsession\.unit\s*\(/ },
      { name: "client .getUnit(...)", needle: /\.getUnit\s*\(/ },
      { name: "client .listSessionRecords(...)", needle: /\.listSessionRecords\s*\(/ },
      { name: "client .readSessionFileText(...)", needle: /\.readSessionFileText\s*\(/ },
      { name: "client .waitForRun(...)", needle: /\.waitForRun\s*\(/ },
      // Removed `RuntimeSizes` export (use `Sizes`).
      { name: "RuntimeSizes export", needle: /\bRuntimeSizes\b/ },
      // Renamed data-source chat tools (run vocabulary -> session vocabulary).
      { name: "list_sessions tool", needle: /\blist_sessions\b/ },
      { name: "get_session tool", needle: /\bget_run\b/ },
      { name: "session_id tool arg", needle: /\bsession_id\b/ },
      // Removed LegacySessionRef / ref-style run API.
      { name: "legacy ref type", needle: /\bLegacySessionRef\b/ },
      { name: "ref.sessionId", needle: /\bref\.sessionId\b/ },
      { name: "runAndCollect alias", needle: /\brunAndCollect\b/ },
      { name: "secrets.get_value plaintext read", needle: /\.secrets\.get_value\s*\(/ },
      {
        name: "legacy root workspace resource namespace",
        needle: /\baex\.(?:files|skills|tools|instructions|secrets)\b/
      },
      {
        name: "callable session resource namespace",
        needle: /\bsession\.(?:messages|events|files|webhooks)\s*\(/
      },
      { name: "internal run-finalization event", needle: /\baex\.run\.finalizing\b/ },
      { name: "removed Secret.upload promotion", needle: /\bSecret\.value\([^\n]*\)\.upload\s*\(/ },
      {
        name: "ref method",
        needle: /\bref\.(?:get|getUnit|events|stream|streamEnvelopes|wait|files|download|downloadSessionFile|downloadFiles|downloadEvents|downloadMetadata|cancel|delete)\s*\(/
      },
      // The session read/stream/download surface moved from flat handle methods
      // onto accessor sub-resources (`session.events.list()`,
      // `session.files.read(...)`, `session.messages.last()`, …). The docs
      // must not resurrect the removed flat form invoked directly on a handle.
      // `session.download(...)` / `session.downloadMetadata(...)` stay flat, and
      // `session.events.streamEnvelopes(...)` (accessor form) is allowed —
      // only the direct `session.streamEnvelopes(...)` is forbidden.
      {
        name: "flat session-handle read/stream/download verb (now under messages/events/files/webhooks)",
        needle:
          /\b(?:session|resumed|handle)\.(?:listEvents|streamEvents|streamEnvelopes|eventArchiveLink|downloadEvents|listFiles|readSessionFile|findFiles|findSessionFile|sessionFileLink|fetchSessionFile|downloadFiles|downloadSessionFile|webhookDeliveries|redeliverWebhook)\s*\(/
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
  /^aex\s+(run|login|logout|whoami|auth|models|providers|tools|runtime-sizes|events|tail|inspect|wait|status|files|download|cancel|delete|delete-asset|billing|webhooks|sessions|sessions|deliveries)\b/m;

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

  it("the quickstart mints billing:read alongside the sessions/files scopes", () => {
    const quickstart = readDoc("packages/sdk/docs/quickstart.md");
    for (const scope of ["sessions:read", "sessions:write", "files:read", "billing:read"]) {
      expect(quickstart).toContain(scope);
    }
  });

  it("files.md carries NO baseline-snapshot / filesystem-diff capture fiction", () => {
    const files = readDoc("packages/sdk/docs/files.md");
    const fictions = [
      /snapshots the filesystem/i,
      /filesystem diff/i,
      /uploads the delta/i,
      /baseline snapshot/i,
      /excluded by timing/i
    ];
    const hits = fictions.filter((f) => f.test(files)).map((f) => f.source);
    expect(hits).toEqual([]);
    // It still teaches that a file identity is bound to one checkpoint.
    expect(files).toMatch(/file IDs?[^.]*checkpoint|ID selectors?[^.]*checkpoint/i);
  });

  it("public file-capture docs and comments describe latest-checkpoint files", () => {
    const checked = [
      "README.md",
      "packages/sdk/docs/concepts/sessions.md",
      "packages/sdk/docs/public-surface.json",
      "packages/sdk/README.md",
      "packages/sdk/src/client.ts",
      "packages/contracts/src/submission.ts"
    ];
    const fictions = [
      /filesystem delta/i,
      /created or modified/i,
      /capture all created\/modified files/i,
      /output modes/i
    ];
    const hits = checked.flatMap((rel) => {
      const text = readDoc(rel);
      return fictions.filter((f) => f.test(text)).map((f) => `${rel}: ${f.source}`);
    });
    expect(hits).toEqual([]);
    expect(readDoc("packages/sdk/docs/concepts/sessions.md")).toMatch(/session is a resumable thread/i);
    expect(readDoc("packages/contracts/src/submission.ts")).toContain("latest complete checkpoint");
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
    expect(errors).toMatch(/empty[^.]*idempotencyKey[^.]*throws|idempotencyKey[^.]*throws[^.]*SessionConfigValidationError/i);
  });

  it("events.md documents capability-honest streaming (typed reject, not silent downgrade)", () => {
    const events = readDoc("packages/sdk/docs/events.md");
    expect(events).toContain("outputMode");
    expect(events).toMatch(/streamable|providers that do not support/i);
    expect(events).toMatch(/typed rejection|fail-closed|silent(?:ly)? downgrade(?:d)?/i);
  });

  it("the custom-tool entry rule and sandbox notes are documented", () => {
    const tools = readDoc("packages/sdk/docs/concepts/agent-tools.md");
    expect(tools).toMatch(/\.mjs/);
    expect(tools).toMatch(/default-export|export default/i);

    const limits = readDoc("packages/sdk/docs/limits-and-quotas.md");
    expect(limits).toContain("maxTurns");
    expect(limits).toMatch(/\/workspace/);
    expect(limits).toMatch(/PEP 668/);
  });

  it("documents the grouped runtime selector from the public contracts", () => {
    const providers = readDoc("packages/sdk/docs/concepts/providers-and-runtimes.md");
    const limits = readDoc("packages/sdk/docs/limits.md");
    const capabilities = readDoc("packages/sdk/docs/provider-runtime-capabilities.md");
    const docs = `${providers}\n${limits}\n${capabilities}`;

    expect(docs).not.toMatch(/no alternative runtime backend|no public runtime selector/i);
    expect(providers).toContain("runtime.kind");
    expect(providers).toContain("runtime.size");
    expect(providers).toContain("--runtime <kind>");
    expect(limits).toContain("runtime.kind");
    expect(limits).toContain("runtime.size");
    expect(capabilities).toContain("runtime.kind");
    expect(capabilities).toContain("runtime.size");
    expect(docs).toContain(DEFAULT_RUNTIME_KIND);
    for (const kind of RUNTIME_KINDS) {
      expect(docs).toContain(kind);
    }
  });
});
