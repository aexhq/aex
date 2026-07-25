#!/usr/bin/env bun
/**
 * P7 — emit TypeScript types from the GENERATED OpenAPI documents.
 *
 * This is the tail of the pipeline, not its source of truth. Our own types are
 * already inferred from the same schemas the server validates with, so there is
 * nothing here to reconcile; what this buys (D5) is a language-neutral,
 * publishable description of the wire that a third party — or a future
 * non-TypeScript SDK — can consume without re-deriving the surface by hand.
 *
 * `openapi-typescript` and NOT Orval / `openapi-fetch`: a generated *client*
 * would be a second implementation of `HttpClient` + `operations.ts`, which
 * already own idempotency resolution, transient retry, the structured error
 * factory, SSE handling, archive assembly, and redaction. That is the
 * duplication-by-mirror root cause re-imported as a tool. This emits types
 * only — no runtime, no second client.
 *
 * The output is committed so a reader can diff it, and `--check` fails when the
 * committed copy and a fresh generation disagree — the same generate/check pair
 * as `openapi:generate` / `openapi:check` and `capabilities:generate` /
 * `capabilities:check`.
 *
 * Usage:
 *   bun scripts/openapi/types.ts            # write
 *   bun scripts/openapi/types.ts --check    # fail if committed output is stale
 */
import { readFile, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import openapiTS, { astToString } from "openapi-typescript";

const repoRoot = resolve(fileURLToPath(new URL("../..", import.meta.url)));
const openapiDir = resolve(repoRoot, "packages/contracts/openapi");

interface TypedPlane {
  /** Generated OpenAPI document, relative to `packages/contracts/openapi`. */
  readonly spec: string;
  /** Committed declaration file, alongside the document it describes. */
  readonly types: string;
}

const PLANES: readonly TypedPlane[] = [{ spec: "data-plane.json", types: "data-plane.d.ts" }];

function banner(plane: TypedPlane): string {
  return [
    "/**",
    " * GENERATED FILE — DO NOT EDIT.",
    " *",
    ` * Emitted by \`bun run openapi:types:generate\` from ./${plane.spec} with`,
    " * openapi-typescript. That document is itself generated from the schemas in",
    " * packages/contracts/src/schemas/** and the route table in src/api-routes.ts,",
    " * so this file is two derivations away from the code the server runs and zero",
    " * derivations away from anything hand-maintained.",
    " *",
    " * `openapi:types:check` fails when this file and a fresh generation disagree.",
    " */",
    ""
  ].join("\n");
}

/** Line endings are normalised before comparison — see the `--check` note in main(). */
function normalizeEol(text: string): string {
  return text.replace(/\r\n/g, "\n");
}

async function renderTypes(plane: TypedPlane): Promise<string> {
  const document: unknown = JSON.parse(await readFile(resolve(openapiDir, plane.spec), "utf8"));
  // The document is passed as an object rather than a path: every `$ref` in it
  // is internal, so there is nothing to fetch, and this keeps the generation
  // independent of the caller's working directory.
  const ast = await openapiTS(document as Parameters<typeof openapiTS>[0], {
    // The document still carries properties it cannot describe yet (P2's field
    // types are outstanding), and those arrive here as empty schemas. `unknown`
    // forces a consumer to narrow; the default `Record<string, never>` would
    // claim, falsely, that no value is permitted.
    emptyObjectsUnknown: true
  });
  return `${banner(plane)}${astToString(ast)}`.replace(/\n*$/, "\n");
}

async function main(): Promise<number> {
  const check = process.argv.includes("--check");

  let stale = false;
  for (const plane of PLANES) {
    const rendered = await renderTypes(plane);
    const target = resolve(openapiDir, plane.types);
    if (check) {
      const existing = await readFile(target, "utf8").catch(() => "");
      // Compare with line endings normalised, for the same reason
      // `openapi:check` does: `.gitattributes` pins these files to LF so the
      // working tree is canonical, but a checkout made before that landed — or
      // one with a local `core.autocrlf` — would otherwise report a clean tree
      // as stale, and a gate that cries wolf is a gate people learn to skip.
      if (normalizeEol(existing) !== normalizeEol(rendered)) {
        console.error(
          `openapi:types:check FAILED — ${plane.types} is stale. Run \`bun run openapi:types:generate\`.`
        );
        stale = true;
      }
      continue;
    }
    await writeFile(target, rendered, "utf8");
    console.log(`openapi:types: wrote ${plane.types}`);
  }

  if (check && !stale) {
    console.log(`openapi:types:check OK: ${PLANES.length} declaration file(s) match their documents.`);
  }
  return stale ? 1 : 0;
}

if (import.meta.main) {
  process.exit(await main());
}
