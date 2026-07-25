/**
 * Identifier-format parity for the PUBLIC repository.
 *
 * `packages/contracts/src/ids.ts` is the id authority: it declares every
 * prefix, mints every value, and exports the only parser. `references/rules.md`
 * § *Identifier ownership* says "no re-declared prefix literal, no hand-written
 * shape regex, no format restated in a comment, doc, or test fixture" — this is
 * the check that makes that true here.
 *
 * The platform repository has the same gate at
 * `platform/scripts/validate/id-format-parity.test.ts`, and it cannot cover this
 * tree: the two repositories are separate git remotes, and the platform gate
 * cannot assume a public checkout exists. So the public repo enforces its own
 * shapes, over its own roots, with its own runner (`bun test`, collected by
 * `scripts/cicd/run-validation-tests.mjs`).
 *
 * Three scanned properties plus the owner's own invariants, each failing on a
 * deliberate violation:
 *   1. No `^<prefix>_` shape literal in source outside the owner — regex
 *      literal, `new RegExp("…")`, or JSON-Schema `"pattern"` alike, since all
 *      three carry the same anchored text.
 *   2. No second generator: `randomUUID` never mints an entity id.
 *   3. Every declared kind mints exactly one form, and refuses its neighbours.
 *
 * DELIBERATELY out of scope:
 *   - `packages/contracts/src/session-config.ts`'s `SKILL_ID_PATTERN`
 *     (`^skl_[A-Za-z0-9_-]{8,128}$`) and `SKILL_NAME_PATTERN`. `skl` has no
 *     entry in `ID_PREFIXES`, so the anchored scan cannot reach either one; the
 *     skill-bundle id is owned by its own server-side CHECK constraint, not by
 *     this module. Asserted below so the exclusion is a decision, not a gap.
 *   - `randomUUID` for non-entity values: a request id, a lease owner, a
 *     continuation token, a temp-file suffix. Those are not identifiers of
 *     anything the id owner names.
 */
import { readFileSync, readdirSync } from "node:fs";
import { join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "bun:test";
// A relative source import, the convention every other `scripts/validate` suite
// uses (`canonical-api-docs.test.ts`, `runtime-capabilities.test.ts`): the
// validation tree runs with cwd pinned inside `scripts/validate`, where the
// workspace package name does not resolve.
import {
  ID_KINDS,
  ID_PREFIXES,
  idPatternSource,
  isId,
  newId
} from "../../packages/contracts/src/ids.js";

const repoRoot = resolve(fileURLToPath(new URL("../..", import.meta.url)));

/** Generated, vendored, or build output — none of it is authored here. */
const SKIP_DIRECTORIES = new Set([
  "node_modules",
  "dist",
  ".next",
  ".tmp",
  ".turbo",
  "coverage",
  "_generated"
]);

const SOURCE_EXTENSIONS = [".ts", ".tsx", ".mts", ".cts", ".mjs", ".cjs", ".js"];

/**
 * The scanned roots. `references/**` and `apps/docs/content/**` are prose — a
 * doc naming a shape is a documentation-drift concern, owned by the docs
 * generator and its own checks.
 */
const SCAN_ROOTS = ["packages", "apps", "scripts", "tools"];

/** Files whose whole job is to state the shape, and therefore may. */
const ALLOWED_SHAPE_LITERAL_FILES = new Set([
  // The owner. Every other pattern in this repository is derived from it.
  "packages/contracts/src/ids.ts",
  // The owner's own test: pinning `idPatternSource` against the literal string
  // is the assertion. Deriving it from the thing under test would prove nothing.
  "packages/contracts/test/ids.test.ts",
  // This gate: it must name the shapes it forbids elsewhere.
  "scripts/validate/id-format-parity.test.ts"
]);

interface ShapeHit {
  readonly file: string;
  readonly line: number;
  readonly literal: string;
}

/**
 * `^<prefix>_` in a regex literal, a `new RegExp("...")`, or a JSON-Schema
 * `pattern`. Deliberately matches the ANCHORED form only: an unanchored mention
 * of a prefix inside a message string is not a shape declaration.
 */
function shapeLiteralPattern(): RegExp {
  return new RegExp(String.raw`\^(?:${Object.values(ID_PREFIXES).join("|")})_`, "g");
}

function walkSourceFiles(root: string): string[] {
  let entries: ReturnType<typeof readdirSync>;
  try {
    entries = readdirSync(root, { withFileTypes: true });
  } catch {
    return [];
  }
  return entries.flatMap((entry) => {
    const path = join(root, entry.name);
    if (entry.isDirectory()) {
      return SKIP_DIRECTORIES.has(entry.name) ? [] : walkSourceFiles(path);
    }
    if (!entry.isFile()) return [];
    return SOURCE_EXTENSIONS.some((extension) => entry.name.endsWith(extension)) ? [path] : [];
  });
}

function scannedFiles(): { readonly path: string; readonly relativePath: string }[] {
  const files: { path: string; relativePath: string }[] = [];
  for (const root of SCAN_ROOTS) {
    for (const path of walkSourceFiles(resolve(repoRoot, root))) {
      const relativePath = relative(repoRoot, path).split("\\").join("/");
      if (ALLOWED_SHAPE_LITERAL_FILES.has(relativePath)) continue;
      files.push({ path, relativePath });
    }
  }
  return files;
}

function scanLines(match: (text: string) => boolean): string[] {
  const found: string[] = [];
  for (const file of scannedFiles()) {
    readFileSync(file.path, "utf8").split("\n").forEach((text, index) => {
      if (match(text)) found.push(`${file.relativePath}:${index + 1}`);
    });
  }
  return found;
}

function scanForShapeLiterals(): ShapeHit[] {
  const pattern = shapeLiteralPattern();
  const hits: ShapeHit[] = [];
  for (const file of scannedFiles()) {
    readFileSync(file.path, "utf8").split("\n").forEach((text, index) => {
      pattern.lastIndex = 0;
      for (const found of text.matchAll(pattern)) {
        hits.push({ file: file.relativePath, line: index + 1, literal: found[0]! });
      }
    });
  }
  return hits;
}

describe("no hand-written id shape outside the owner", () => {
  it("finds no `^<prefix>_` literal in public source", () => {
    const offenders = scanForShapeLiterals().map(
      (hit) => `${hit.file}:${hit.line} declares ${hit.literal}… — import isId/idPattern from @aexhq/contracts instead`
    );
    expect(offenders).toEqual([]);
  });

  it("would catch a deliberate violation for every declared kind", () => {
    // The check is only worth having if it fails on the thing it forbids.
    const pattern = shapeLiteralPattern();
    for (const kind of ID_KINDS) {
      pattern.lastIndex = 0;
      expect(`const RE = /^${ID_PREFIXES[kind]}_[0-9a-f]{32}$/i;`).toMatch(pattern);
      pattern.lastIndex = 0;
      expect(`pattern: "^${ID_PREFIXES[kind]}_[0-9a-f]{32}$"`).toMatch(pattern);
    }
  });

  it("does not reach the skill patterns, whose prefix this module does not own", () => {
    // `SKILL_ID_PATTERN` / `SKILL_NAME_PATTERN` in
    // packages/contracts/src/session-config.ts describe a skill bundle, whose
    // `skl` prefix has no `ID_PREFIXES` entry. They are out of this gate's scope
    // by construction rather than by exemption — adding `skl` to the owner would
    // bring them in, which is the correct way for that to change.
    const pattern = shapeLiteralPattern();
    for (const outOfScope of [
      "export const SKILL_ID_PATTERN = /^skl_[A-Za-z0-9_-]{8,128}$/;",
      "export const SKILL_NAME_PATTERN = /^[a-z0-9][a-z0-9_-]{0,127}$/;"
    ]) {
      pattern.lastIndex = 0;
      expect(outOfScope).not.toMatch(pattern);
    }
    expect(Object.values(ID_PREFIXES)).not.toContain("skl");
  });
});

describe("one generator per entity", () => {
  /**
   * `randomUUID` adjacent to an id prefix. Legitimate `randomUUID` (request ids,
   * lease owners, temp paths) never appears immediately after a `<prefix>_`.
   */
  function mintingPattern(): RegExp {
    return new RegExp(String.raw`(?:${Object.values(ID_PREFIXES).join("|")})_\$\{[^}]*randomUUID`, "g");
  }

  it("mints no entity id from randomUUID", () => {
    const minting = mintingPattern();
    const offenders = scanLines((text) => {
      minting.lastIndex = 0;
      return minting.test(text);
    }).map((location) => `${location} mints an id from randomUUID — use newId(kind)`);
    expect(offenders).toEqual([]);
  });

  it("would catch a deliberate violation, and leaves non-entity uuids alone", () => {
    const minting = mintingPattern();
    for (const violation of [
      "return `sec_${randomUUID()}`;",
      'return `wres_${randomUUID().replaceAll("-", "")}`;'
    ]) {
      minting.lastIndex = 0;
      expect(violation).toMatch(minting);
    }
    for (const legitimate of [
      "const requestId = randomUUID();",
      "const ownerId = randomUUID();",
      "const tempPath = `${dir}/${randomUUID()}.tmp`;"
    ]) {
      minting.lastIndex = 0;
      expect(legitimate).not.toMatch(minting);
    }
  });
});

describe("the owner is the only mint", () => {
  it("mints one form per kind, and refuses every neighbouring form", () => {
    for (const kind of ID_KINDS) {
      const minted = newId(kind);
      expect(minted, kind).toMatch(new RegExp(idPatternSource(kind)));
      expect(isId(kind, minted)).toBe(true);
      // Case-sensitive by design: `newId` mints lowercase hex, so an uppercase
      // id is a different string, not an alternative encoding of the same one.
      expect(isId(kind, minted.toUpperCase())).toBe(false);
      // No dashed or prefix-stripped twin is admitted for any kind.
      expect(isId(kind, minted.slice(ID_PREFIXES[kind].length + 1))).toBe(false);
    }
  });

  it("keeps every kind's prefix distinct, so idKindOf is unambiguous", () => {
    const prefixes = Object.values(ID_PREFIXES);
    expect(new Set(prefixes).size).toBe(prefixes.length);
  });
});
