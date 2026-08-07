import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, test } from "bun:test";

import { AexConfigError, WorkspaceApiKey } from "../../src/index.js";

interface WorkspaceKeyCase {
  readonly reason: string;
  readonly accepted: boolean;
  readonly text: string;
  readonly region?: string;
  readonly workspace?: string;
  readonly key?: string;
}

const CORPUS = join(
  dirname(fileURLToPath(import.meta.url)),
  "../../../../conformance/credentials/workspace-keys.jsonl",
);

function cases(): WorkspaceKeyCase[] {
  const text = readFileSync(CORPUS, "utf8");
  const parsed = text
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.length > 0 && !line.startsWith("#"))
    .map((line) => JSON.parse(line) as WorkspaceKeyCase);
  if (parsed.length === 0) {
    throw new Error(`the credential corpus at ${CORPUS} is empty; an empty case set is a silent skip`);
  }
  return parsed;
}

describe("workspace key grammar agreement", () => {
  // The same bytes the Rust parser is held to. Two parsers that disagree about
  // which segment is the workspace would authorize against the wrong rows, so
  // the identities are asserted rather than only the accept/reject verdict.
  test("the corpus verdicts and identities match", () => {
    let accepted = 0;
    let rejected = 0;
    for (const entry of cases()) {
      if (entry.accepted) {
        const key = WorkspaceApiKey.parse(entry.text);
        expect(key.regionCode(), entry.reason).toBe(entry.region);
        expect(key.workspaceId(), entry.reason).toBe(`wsp_${entry.workspace}`);
        accepted += 1;
      } else {
        expect(() => WorkspaceApiKey.parse(entry.text), entry.reason).toThrow(AexConfigError);
        rejected += 1;
      }
    }
    expect(accepted).toBeGreaterThanOrEqual(3);
    expect(rejected).toBeGreaterThanOrEqual(10);
  });
});
