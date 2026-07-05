/**
 * WS6: OutputSearchQuery.filename and OutputQuery.filename are the SAME type
 * (`string | RegExp`), and one shared `toFilenameMatcher` never passes a RegExp
 * into a string-only path (the T16 crash class).
 */
import { describe, expect, it } from "vitest";
import { operations, type OutputQuery, type OutputSearchQuery } from "../src/index.js";

describe("unified filename query type (WS6)", () => {
  it("[compile-time] OutputSearchQuery.filename and OutputQuery.filename are interchangeable", () => {
    const re: OutputQuery["filename"] = /report/i;
    const searchFromQuery: OutputSearchQuery["filename"] = re;
    const queryFromSearch: OutputQuery["filename"] = searchFromQuery;
    const str: OutputSearchQuery["filename"] = "notes.txt";
    expect([re, searchFromQuery, queryFromSearch, str].length).toBe(4);
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
