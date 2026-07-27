import { existsSync, readdirSync, statSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "bun:test";
import { parse } from "yaml";
import { readRepoFile } from "./workflow-test-helpers.js";

/**
 * Every `uses:` reference must name an immutable commit SHA.
 *
 * A major-version tag (`oven-sh/setup-bun@v2`) is a mutable pointer owned by
 * the action's author. Whoever controls that account can retarget it at any
 * commit, and every workflow in this repository picks the new code up on its
 * next run — including the publish job, which holds `id-token: write` and can
 * therefore mint an npm OIDC token and publish `@aexhq/sdk` with a VALID
 * provenance attestation, because this workflow genuinely produced it.
 * Provenance attests to the builder, not to the builder's integrity.
 *
 * A 40-character SHA cannot be retargeted, so the pin is the control.
 * .github/dependabot.yml is the other half: it moves the pins forward so
 * "immutable" does not decay into "unpatched".
 *
 * The ONLY exemption is a local reference (`uses: ./...`), which resolves
 * inside this checkout at the reviewed commit and has no upstream owner. It is
 * asserted explicitly below rather than left to fall through the SHA regex.
 */
const repoRoot = fileURLToPath(new URL("../..", import.meta.url));
const commitShaPin = /@[0-9a-f]{40}$/;
const localReference = /^\.\//;

/** `- uses: owner/repo@ref  # tag`, quoted or not, at any indentation. */
const usesLine = /^\s*(?:-\s+)?uses:\s*(?:"([^"]*)"|'([^']*)'|([^#\s]+))\s*(?:#\s*(\S.*?))?\s*$/;

interface ActionReference {
  readonly file: string;
  readonly line: number;
  readonly value: string;
  readonly comment: string | undefined;
}

/** Workflows plus any composite/local actions, both of which carry `uses:`. */
function actionDefinitionPaths(): readonly string[] {
  const paths: string[] = [];

  const workflows = resolve(repoRoot, ".github/workflows");
  if (existsSync(workflows)) {
    for (const name of readdirSync(workflows)) {
      if (name.endsWith(".yml") || name.endsWith(".yaml")) paths.push(`.github/workflows/${name}`);
    }
  }

  // .github/actions/** does not exist today; discover it rather than assume,
  // so a composite action added later inherits the gate instead of escaping it.
  const actions = resolve(repoRoot, ".github/actions");
  if (existsSync(actions)) {
    const walk = (relative: string): void => {
      for (const entry of readdirSync(resolve(repoRoot, relative))) {
        const child = `${relative}/${entry}`;
        if (statSync(resolve(repoRoot, child)).isDirectory()) walk(child);
        else if (entry === "action.yml" || entry === "action.yaml") paths.push(child);
      }
    };
    walk(".github/actions");
  }

  return paths.sort();
}

function scanReferences(paths: readonly string[]): readonly ActionReference[] {
  const references: ActionReference[] = [];
  for (const file of paths) {
    readRepoFile(file)
      .split("\n")
      .forEach((text, index) => {
        const match = usesLine.exec(text);
        if (!match) return;
        references.push({
          file,
          line: index + 1,
          value: match[1] ?? match[2] ?? match[3] ?? "",
          comment: match[4]
        });
      });
  }
  return references;
}

/** Independent count from the parsed document, so the line scanner cannot under-report. */
function parsedReferenceCount(paths: readonly string[]): number {
  let total = 0;
  const walk = (node: unknown): void => {
    if (Array.isArray(node)) {
      for (const item of node) walk(item);
      return;
    }
    if (node === null || typeof node !== "object") return;
    for (const [key, value] of Object.entries(node as Record<string, unknown>)) {
      if (key === "uses" && typeof value === "string") total += 1;
      else walk(value);
    }
  };
  for (const file of paths) walk(parse(readRepoFile(file)));
  return total;
}

const definitionPaths = actionDefinitionPaths();
const references = scanReferences(definitionPaths);

function describeReference(reference: ActionReference): string {
  return `${reference.file}:${reference.line} ${reference.value}`;
}

describe("GitHub Action pins", () => {
  it("scans every workflow and composite action definition", () => {
    expect(definitionPaths.length, "no .github action definitions were discovered").toBeGreaterThan(0);
    expect(references.length, "no `uses:` references were discovered").toBeGreaterThan(0);
    expect(references.length, `line scan missed a \`uses:\` in ${definitionPaths.join(", ")}`).toBe(
      parsedReferenceCount(definitionPaths)
    );
  });

  it("pins every third-party action to a 40-character commit SHA", () => {
    const unpinned = references
      .filter((reference) => !localReference.test(reference.value))
      .filter((reference) => !commitShaPin.test(reference.value))
      .map(describeReference);

    expect(
      unpinned,
      `${unpinned.length} of ${references.length} \`uses:\` references are on a mutable ref:\n${unpinned.join("\n")}`
    ).toEqual([]);
  });

  it("exempts only local references, and only when the referenced path exists", () => {
    for (const reference of references.filter((candidate) => localReference.test(candidate.value))) {
      const [path] = reference.value.split("@");
      expect(existsSync(resolve(repoRoot, path ?? "")), `${describeReference(reference)} does not exist`).toBe(true);
    }
  });

  it("records the human-readable tag beside every pin", () => {
    const undocumented = references
      .filter((reference) => commitShaPin.test(reference.value))
      .filter((reference) => reference.comment === undefined)
      .map(describeReference);

    expect(
      undocumented,
      `every SHA pin needs a trailing \`# <tag>\` comment so it stays reviewable and Dependabot-updatable:\n${undocumented.join("\n")}`
    ).toEqual([]);
  });
});
