/**
 * The generated-docs banner gate.
 *
 * WHY. `apps/docs/content/docs/` is written by `scripts/docs/generate-all.mjs`,
 * and eleven of its outputs are also tracked in Git while their siblings are
 * gitignored. Nothing in those files said "generated", so an external
 * documentation pull request against one of them would be reviewed, merged, and
 * then silently reverted by the next `bun run lint` — which regenerates through
 * `prelint` before it lints anything.
 *
 * This asserts three separable things:
 *   1. every TRACKED file the generator writes carries the marker;
 *   2. every file the generator writes at all carries it, tracked or not, so
 *      the rule does not depend on a `.gitignore` staying as it is;
 *   3. no HAND-AUTHORED page in the same directories carries it, which is what
 *      would happen if someone "fixed" a failure by pasting the banner instead
 *      of moving their edit to the source.
 *
 * The tracked set is derived from `git ls-files`, not hard-coded: the point of
 * the gate is to notice when the set changes.
 */
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "bun:test";

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));
const contentRoot = resolve(repoRoot, "apps/docs/content/docs");
const generator = resolve(repoRoot, "scripts/docs/generate-all.mjs");

/** Must stay byte-identical to `GENERATED_DOC_MARKER` in the generator. */
const MARKER = "GENERATED FILE - do not edit";

/**
 * Written by the generator. Derived from the generator source where that is
 * possible and listed where it is not: the guide and concept slugs come from
 * the same arrays the generator iterates, so a new page cannot be added there
 * without this list following.
 */
function generatedRelativePaths(): string[] {
  const source = readFileSync(generator, "utf8");
  const slugs = (name: string): string[] => {
    const block = new RegExp(`const ${name} = \\[([\\s\\S]*?)\\n\\];`).exec(source);
    if (block?.[1] === undefined) throw new Error(`generator no longer declares ${name}`);
    return [...block[1].matchAll(/\[\s*"[^"]+",\s*"([^"]+)"\s*\]/g)]
      .map((match) => match[1])
      .filter((slug): slug is string => slug !== undefined);
  };

  return [
    "index.md",
    "features.md",
    "meta.json",
    "concepts/meta.json",
    ...slugs("conceptSources").map((slug) => `concepts/${slug}.md`),
    "guides/meta.json",
    ...slugs("guideSources").map((slug) => `guides/${slug}.md`),
    "reference/meta.json",
    "reference/provider-runtime-capabilities.md",
    "reference/cli.md",
    "reference/api.md",
    "reference/events.md",
    "reference/sdk/index.md"
  ];
}

function trackedUnder(path: string): string[] {
  return execFileSync("git", ["ls-files", "--", path], { cwd: repoRoot, encoding: "utf8" })
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
}

const generated = generatedRelativePaths();
const generatedSet = new Set(generated);

describe("generated documentation carries a do-not-edit banner", () => {
  it("finds the generator's output set", () => {
    // The derivation above reads the generator source. If it silently resolved
    // to nothing, every assertion below would pass vacuously.
    expect(generated.length).toBeGreaterThan(25);
    expect(generated).toContain("guides/quickstart.md");
    expect(generated).toContain("concepts/sessions.md");
  });

  it("banners every generated file present on disk", () => {
    // Present-on-disk rather than all-of-them: the suite must pass in a clean
    // checkout that has not run the generator yet. The tracked subset below is
    // the part that is always there.
    const unbannered = generated
      .map((relativePath) => resolve(contentRoot, relativePath))
      .filter((path) => existsSync(path))
      .filter((path) => !readFileSync(path, "utf8").includes(MARKER))
      .map((path) => relative(repoRoot, path).replaceAll("\\", "/"))
      .sort();

    expect(unbannered).toEqual([]);
  });

  it("banners every generated file that is also tracked in Git", () => {
    // This is the case the gate exists for: a tracked generated file looks
    // hand-authored in a diff and in a code-review UI.
    const trackedGenerated = trackedUnder("apps/docs/content/docs")
      .map((path) => path.slice("apps/docs/content/docs/".length))
      .filter((path) => generatedSet.has(path));

    expect(trackedGenerated.length).toBeGreaterThan(0);

    const unbannered = trackedGenerated
      .filter((path) => !readFileSync(resolve(contentRoot, path), "utf8").includes(MARKER))
      .sort();

    expect(unbannered).toEqual([]);
  });

  it("leaves hand-authored pages unbannered", () => {
    // The wrong fix for a failure above is pasting the banner into a page a
    // human owns. These four plus the reference index are hand-authored.
    const handAuthored = trackedUnder("apps/docs/content/docs")
      .map((path) => path.slice("apps/docs/content/docs/".length))
      .filter((path) => !generatedSet.has(path));

    expect(handAuthored.sort()).toEqual([
      "changelog.md",
      "examples.md",
      "integrations.md",
      "reference/index.md",
      "support.md"
    ]);

    const wronglyBannered = handAuthored
      .filter((path) => readFileSync(resolve(contentRoot, path), "utf8").includes(MARKER))
      .sort();

    expect(wronglyBannered).toEqual([]);
  });

  it("keeps the banner out of the rendered page", () => {
    // The markdown banner is a YAML COMMENT inside frontmatter and the meta
    // banner is a `$generated` key a non-strict Zod object strips. Neither may
    // become body text, or every docs page grows a visible warning.
    const page = readFileSync(resolve(contentRoot, "concepts/sessions.md"), "utf8");
    const [, frontmatter = "", ...rest] = page.split(/^---\r?$/m);

    expect(frontmatter).toContain(MARKER);
    expect(rest.join("")).not.toContain(MARKER);
    for (const line of frontmatter.split("\n").filter((entry) => entry.includes(MARKER))) {
      expect(line.trimStart().startsWith("#")).toBe(true);
    }
  });

  it("keeps the generator and this gate on the same marker string", () => {
    const source = readFileSync(generator, "utf8");

    expect(source).toContain(`const GENERATED_DOC_MARKER = "${MARKER}"`);
  });
});
