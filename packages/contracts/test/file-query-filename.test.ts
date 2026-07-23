/**
 * Session file selectors accept `string | RegExp`, and the shared matcher never
 * passes a RegExp into a string-only path.
 */
import { describe, expect, it } from "bun:test";
import type { SessionFileQuery } from "../src/index.js";
import { operations } from "../src/internal.js";

describe("unified filename query type (WS6)", () => {
  it("[compile-time] SessionFileQuery.filename accepts strings and regular expressions", () => {
    const re: SessionFileQuery["filename"] = /report/i;
    const str: SessionFileQuery["filename"] = "notes.txt";
    expect([re, str].length).toBe(2);
  });

  it("toFilenameMatcher: string is a case-insensitive substring match", () => {
    const match = operations.toFilenameMatcher("report");
    expect(match("Q3-REPORT.md")).toBe(true);
    expect(match("summary.md")).toBe(false);
  });

  it("toFilenameMatcher: a RegExp is tested as-is (no crash)", () => {
    const match = operations.toFilenameMatcher(/^data-\d+\.json$/);
    expect(match("data-42.json")).toBe(true);
    expect(match("data.json")).toBe(false);
  });

  it("toFilenameMatcher: a reused global RegExp does not desync via lastIndex", () => {
    const match = operations.toFilenameMatcher(/a/g);
    expect(match("a")).toBe(true);
    expect(match("a")).toBe(true);
  });
});
