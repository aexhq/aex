import { describe, expect, it } from "bun:test";
import { crossPlatformBasename } from "../../src/path-basename.js";

describe("crossPlatformBasename", () => {
  it.each([
    ["report.txt", "report.txt"],
    ["/workspace/report.txt", "report.txt"],
    ["/workspace/nested///", "nested"],
    [String.raw`C:\workspace\report.txt`, "report.txt"],
    [String.raw`C:\workspace/mixed\report.txt///`, "report.txt"],
    ["\\\\server\\share\\folder\\", "folder"],
    ["/", ""],
    ["////", ""],
    ["C:\\", "C:"],
    ["", ""]
  ])("normalizes %j to basename %j without host-path semantics", (path, expected) => {
    expect(crossPlatformBasename(path)).toBe(expected);
  });
});
