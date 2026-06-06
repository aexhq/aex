/**
 * [SECURITY/CORRECTNESS REGRESSION] High finding H9 — SDK docs ↔ code drift.
 *
 * Source: code review, 2026-05-23. Top-10 action #6.
 *
 * Issue
 * -----
 * The README and `packages/sdk/docs/*.md` advertise SDK methods that
 * do not exist in the current shipped code:
 *
 *   - `Skill.fromPath(...).upload(client)`           — README:38, skills.md:51
 *   - `Skill.fromFiles(...).upload(client)`          — skills.md:96
 *   - `Skill.fromPath(...).uploadIfChanged(client)`  — skills.md:62
 *   - `Skill.fromId(...)`                            — implied throughout
 *   - `client.skills.findByHash(...)`                — skills.md:74
 *   - `client.skills.findByName(...)`                — skills.md:79
 *   - `client.submitRun(config, options)` (2-arg)    — credentials.md,
 *                                                       run-config.md,
 *                                                       cleanup.md:10
 *
 * `packages/sdk/src/skill.ts:28-30` explicitly states *"There is no
 * `Skill.fromId(...)` and no `.upload(client)`"* — i.e. the doc/code
 * gap is acknowledged in the source comment, but the published docs
 * weren't updated. Every snippet that touches uploads is currently
 * broken; users following the README throw `TypeError: …upload is not
 * a function`.
 *
 * Locked invariant
 * ----------------
 * Either the methods exist (pick the "restore uploads" direction) OR
 * the docs no longer reference them (pick the "draft-only" direction).
 * Either way, the test below is the single tripwire — it stays green by
 * keeping the docs and the shipped SDK surface aligned.
 */

import { describe, expect, it } from "vitest";
import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { resolve, dirname } from "node:path";
import {
  AgentExecutor,
  Skill,
  AgentsMd,
  File as AexFile
} from "../../src/index.js";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "..", "..", "..", "..");
const docsRoot = resolve(repoRoot, "packages", "sdk", "docs");

function readDoc(name: string): string {
  return readFileSync(resolve(docsRoot, name), "utf8");
}

function readReadme(): string {
  return readFileSync(resolve(repoRoot, "README.md"), "utf8");
}

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

function publishedDocFiles(): ReadonlyArray<{ readonly label: string; readonly content: string }> {
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
    content: readFileSync(path, "utf8")
  }));
}

describe("[REGRESSION] H9 — SDK docs ↔ code drift", () => {
  it("every method advertised in the docs exists on the SDK exports (OR no doc advertises a missing one)", () => {
    const missing: string[] = [];

    // Build a manifest of (advertised-method, code-presence) pairs.
    // Each probe is a runtime existence check — we're deliberately
    // looking at methods that may not be on the static type, so we
    // route every read through `unknown` first to satisfy TS's
    // "no implicit overlap" rule on the double-cast.
    const probe = (obj: object, key: string): boolean =>
      typeof (obj as unknown as Record<string, unknown>)[key] === "function";
    const hasKey = (obj: object, key: string): boolean =>
      (obj as unknown as Record<string, unknown>)[key] !== undefined;

    const manifest: ReadonlyArray<{ doc: string; needle: RegExp; present: boolean; method: string }> = [
      // Skill.upload(aex)
      {
        doc: "README.md",
        needle: /Skill\.fromPath\([^)]*\)\.upload\((?:client|aex)\)/,
        present: probe(Skill.prototype, "upload"),
        method: "Skill.prototype.upload"
      },
      // Skill.uploadIfChanged(aex)
      {
        doc: "skills.md",
        needle: /\.uploadIfChanged\((?:client|aex)\)/,
        present: probe(Skill.prototype, "uploadIfChanged"),
        method: "Skill.prototype.uploadIfChanged"
      },
      // aex.skills.findByHash
      {
        doc: "skills.md",
        needle: /(?:client|aex)\.skills\.findByHash\(/,
        present: hasKey(AgentExecutor.prototype, "skills"),
        method: "AgentExecutor.prototype.skills.findByHash"
      },
      // aex.skills.findByName
      {
        doc: "skills.md",
        needle: /(?:client|aex)\.skills\.findByName\(/,
        present: hasKey(AgentExecutor.prototype, "skills"),
        method: "AgentExecutor.prototype.skills.findByName"
      },
      // 2-arg submitRun(config, opts)
      {
        doc: "credentials.md",
        needle: /(?:client|aex)\.submitRun\((?:config|template),/,
        // The signature is `submitRun(options)`; a 2-arg shape would
        // accept run config as the first positional. We probe by calling
        // length on the function.
        present: AgentExecutor.prototype.submitRun.length >= 2,
        method: "AgentExecutor.prototype.submitRun(config, options)"
      }
    ];

    for (const entry of manifest) {
      const docContent =
        entry.doc === "README.md" ? readReadme() : readDoc(entry.doc);
      const advertised = entry.needle.test(docContent);
      if (advertised && !entry.present) {
        missing.push(
          `Doc ${entry.doc} advertises pattern ${entry.needle} but ${entry.method} does not exist at runtime.`
        );
      }
    }

    expect(missing).toEqual([]);
  });

  it("AgentsMd.fromPath and File.fromPath actually exist (referenced in quickstart.md)", () => {
    // Smoke check the AgentsMd / File static factories used in
    // quickstart.md:69-71 ("File.fromPath('./customer-folder/')",
    // "AgentsMd.fromPath('./AGENTS.md')"). If they don't exist, every
    // quickstart copy-paste throws TypeError.
    // The previous shape was `if (/X\.fromPath\(/.test(quickstart))
    // expect(...)` — a silent-skip when the doc gets rewritten or
    // grep'd differently. These are PUBLIC API contracts whose existence
    // is independent of any single doc page; pin them unconditionally.
    // If a future API change removes fromPath, both the docs AND the test
    // can be deleted in the same PR (the doc-drift test below already
    // enforces "documented APIs must exist").
    const quickstart = readDoc("quickstart.md");
    const hasStatic = (cls: object, key: string): boolean =>
      typeof (cls as unknown as Record<string, unknown>)[key] === "function";
    expect(hasStatic(AexFile, "fromPath")).toBe(true);
    expect(hasStatic(AgentsMd, "fromPath")).toBe(true);
    // Sanity: quickstart still references at least one of them (so this
    // file remains relevant). If the doc stops referencing fromPath
    // entirely, the inverse case must be considered explicitly.
    expect(
      /File\.fromPath\(/.test(quickstart) || /AgentsMd\.fromPath\(/.test(quickstart)
    ).toBe(true);
  });

  it("published docs do not advertise the removed RunRef/ref-style run API", () => {
    const forbidden: ReadonlyArray<{ readonly name: string; readonly needle: RegExp }> = [
      { name: "RunRef type", needle: /\bRunRef\b/ },
      { name: "ref.runId", needle: /\bref\.runId\b/ },
      { name: "ref method", needle: /\bref\.(?:get|getUnit|events|stream|streamEnvelopes|wait|outputs|download|downloadOutput|downloadOutputs|downloadEvents|downloadMetadata|cancel|delete)\s*\(/ },
      { name: "const ref submitRun", needle: /\bconst\s+ref\s*=\s*await\s+(?:client|aex)\.submitRun\(/ }
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
