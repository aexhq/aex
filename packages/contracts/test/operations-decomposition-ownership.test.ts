import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { operations } from "../src/internal.js";
import * as sessionFileQuery from "../src/session-file-query.js";

function source(name: string): string {
  return readFileSync(new URL(`../src/${name}`, import.meta.url), "utf8");
}

describe("operations decomposition ownership", () => {
  it("keeps moved operation exports on the exact private query-leaf bindings", () => {
    expect(operations.resolveSessionFileSelector).toBe(sessionFileQuery.resolveSessionFileSelector);
    expect(operations.filterSessionFiles).toBe(sessionFileQuery.filterSessionFiles);
    expect(operations.toFilenameMatcher).toBe(sessionFileQuery.toFilenameMatcher);
    expect(operations.classifySessionFile).toBe(sessionFileQuery.classifySessionFile);
    expect(operations).not.toHaveProperty("isPathSelector");
    expect(operations).not.toHaveProperty("buildSessionArchive");
    expect(operations).not.toHaveProperty("buildSessionFilesArchive");
  });

  it("keeps transport orchestration out of both dependency-light leaves", () => {
    const operationsSource = source("operations.ts");
    const querySource = source("session-file-query.ts");
    const archiveSource = source("session-archive.ts");

    expect(operationsSource).toMatch(/from "\.\/session-file-query\.js"/);
    expect(operationsSource).toMatch(/from "\.\/session-archive\.js"/);
    expect(operationsSource).not.toMatch(/function (?:resolveSessionFileSelector|filterSessionFiles|toFilenameMatcher|classifySessionFile)\b/);
    expect(operationsSource).not.toMatch(/\b(?:zipSync|strToU8)\b|function (?:zipEntries|jsonEntry|jsonlEntry)\b/);

    expect(querySource).not.toMatch(/\bHttpClient\b|\.request\(|from "fflate"|from "node:|from "\.\/operations\.js"/);
    expect(archiveSource).not.toMatch(/\bHttpClient\b|\.request\(|from "node:|from "\.\/operations\.js"/);
    expect(archiveSource).toMatch(/from "fflate"/);
  });

  it("keeps both leaves private while admitting their complete packed closure", () => {
    expect(source("index.ts")).not.toMatch(/session-(?:archive|file-query)/);
    expect(source("internal.ts")).not.toMatch(/session-(?:archive|file-query)/);

    const boundary = JSON.parse(
      readFileSync(new URL("../../../scripts/cicd/public-boundary-baseline.json", import.meta.url), "utf8")
    ) as {
      readonly contractsInline?: {
        readonly allowedModuleFiles?: readonly string[];
        readonly privateModuleMaxBytes?: Readonly<Record<string, number>>;
      };
    };
    expect(boundary.contractsInline?.allowedModuleFiles).toEqual(expect.arrayContaining([
      "./session-archive.js",
      "./session-file-query.js"
    ]));
    expect(boundary.contractsInline?.privateModuleMaxBytes).toMatchObject({
      "session-archive.js": 8192,
      "session-file-query.js": 12288
    });
  });
});
