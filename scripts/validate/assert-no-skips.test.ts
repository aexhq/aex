import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));
const scriptPath = resolve(repoRoot, "scripts/cicd/assert-no-skips.mjs");

function writeReport(dir: string, name: string, content: string): string {
  const path = resolve(dir, name);
  writeFileSync(path, content, "utf8");
  return path;
}

function run(args: string | readonly string[]): { status: number; output: string } {
  const argv = typeof args === "string" ? [args] : [...args];
  try {
    const output = execFileSync(process.execPath, [scriptPath, ...argv], {
      cwd: repoRoot,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"]
    });
    return { status: 0, output };
  } catch (error) {
    const failure = error as { status?: number; stdout?: string; stderr?: string };
    return { status: failure.status ?? 1, output: `${failure.stdout ?? ""}${failure.stderr ?? ""}` };
  }
}

function inTempDir(
  name: string,
  content: string,
  assert: (result: { status: number; output: string }) => void,
  extraArgs: readonly string[] = []
): void {
  const dir = mkdtempSync(resolve(tmpdir(), "aex-no-skips-"));
  try {
    assert(run([...extraArgs, writeReport(dir, name, content)]));
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

const passingVitestJson = JSON.stringify({
  numTotalTests: 2,
  numPendingTests: 0,
  numTodoTests: 0,
  testResults: [
    {
      name: "suite.test.ts",
      assertionResults: [
        { status: "passed", fullName: "does a thing", title: "does a thing" },
        { status: "passed", fullName: "does another", title: "does another" }
      ]
    }
  ]
});

// Real `bun test --reporter=junit --reporter-outfile=...` output (bun 1.3.14).
const passingJunit = `<?xml version="1.0" encoding="UTF-8"?>
<testsuites name="bun test" tests="3" assertions="3" failures="0" skipped="0" time="0.2579468">
  <testsuite name="pass.test.ts" file="pass.test.ts" tests="3" assertions="3" failures="0" skipped="0" time="0" hostname="">
    <testsuite name="alpha suite" file="pass.test.ts" line="3" tests="2" assertions="2" failures="0" skipped="0" time="0" hostname="">
      <testcase name="adds numbers" classname="alpha suite" time="0.000191" file="pass.test.ts" line="4" assertions="1" />
      <testcase name="concats strings" classname="alpha suite" time="0.000132" file="pass.test.ts" line="7" assertions="1" />
    </testsuite>
    <testcase name="top-level passes" classname="" time="0.000054" file="pass.test.ts" line="12" assertions="1" />
  </testsuite>
</testsuites>
`;

// Real bun 1.3.14 output for it.skip / it.skipIf(true) / describe.skip.
const skippedJunit = `<?xml version="1.0" encoding="UTF-8"?>
<testsuites name="bun test" tests="5" assertions="1" failures="0" skipped="4" time="0.152756">
  <testsuite name="skips.test.ts" file="skips.test.ts" tests="5" assertions="1" failures="0" skipped="4" time="0" hostname="">
    <testsuite name="beta suite" file="skips.test.ts" line="3" tests="3" assertions="1" failures="0" skipped="2" time="0" hostname="">
      <testcase name="runs fine" classname="beta suite" time="0.000049" file="skips.test.ts" line="4" assertions="1" />
      <testcase name="explicitly skipped" classname="beta suite" time="0" file="skips.test.ts" line="7" assertions="0">
        <skipped />
      </testcase>
      <testcase name="conditionally skipped" classname="beta suite" time="0" file="skips.test.ts" line="10" assertions="0">
        <skipped />
      </testcase>
    </testsuite>
    <testsuite name="gamma suite skipped wholesale" file="skips.test.ts" line="15" tests="2" assertions="0" failures="0" skipped="2" time="0" hostname="">
      <testcase name="never runs a" classname="gamma suite skipped wholesale" time="0" file="skips.test.ts" line="16" assertions="0">
        <skipped />
      </testcase>
      <testcase name="never runs b" classname="gamma suite skipped wholesale" time="0" file="skips.test.ts" line="19" assertions="0">
        <skipped />
      </testcase>
    </testsuite>
  </testsuite>
</testsuites>
`;

// Real bun 1.3.14 output for `bun test pass.test.ts -t "adds"` — bun marks
// every NON-selected test <skipped/>, so a `-t`-filtered run must be gated with
// the matching --name-pattern or the gate false-fails (Wave-0 finding).
const filteredJunit = `<?xml version="1.0" encoding="UTF-8"?>
<testsuites name="bun test" tests="3" assertions="1" failures="0" skipped="2" time="0.131097">
  <testsuite name="pass.test.ts" file="pass.test.ts" tests="3" assertions="1" failures="0" skipped="2" time="0" hostname="">
    <testsuite name="alpha suite" file="pass.test.ts" line="3" tests="2" assertions="1" failures="0" skipped="1" time="0" hostname="">
      <testcase name="adds numbers" classname="alpha suite" time="0.000066" file="pass.test.ts" line="4" assertions="1" />
      <testcase name="concats strings" classname="alpha suite" time="0" file="pass.test.ts" line="7" assertions="0">
        <skipped />
      </testcase>
    </testsuite>
    <testcase name="top-level passes" classname="" time="0" file="pass.test.ts" line="12" assertions="0">
      <skipped />
    </testcase>
  </testsuite>
</testsuites>
`;

// Real bun 1.3.14 output for it.todo — reported as <skipped message="TODO" />.
const todoJunit = `<?xml version="1.0" encoding="UTF-8"?>
<testsuites name="bun test" tests="3" assertions="1" failures="0" skipped="2" time="0.1233169">
  <testsuite name="todos.test.ts" file="todos.test.ts" tests="3" assertions="1" failures="0" skipped="2" time="0" hostname="">
    <testcase name="passes normally" classname="" time="0.000042" file="todos.test.ts" line="3" assertions="1" />
    <testcase name="todo without body" classname="" time="0" file="todos.test.ts" line="6" assertions="0">
      <skipped message="TODO" />
    </testcase>
    <testcase name="todo with body" classname="" time="0" file="todos.test.ts" line="7" assertions="0">
      <skipped message="TODO" />
    </testcase>
  </testsuite>
</testsuites>
`;

describe("assert-no-skips release gate", () => {
  it("keeps passing a Vitest JSON report with tests and no skips", () => {
    inTempDir("report.json", passingVitestJson, (result) => {
      expect(result.status).toBe(0);
      expect(result.output).toContain("[assert-no-skips] OK: 2 tests");
    });
  });

  it("keeps failing a Vitest JSON report with a skipped test", () => {
    inTempDir(
      "report.json",
      JSON.stringify({
        numTotalTests: 1,
        testResults: [
          { name: "suite.test.ts", assertionResults: [{ status: "skipped", fullName: "skipped one", title: "skipped one" }] }
        ]
      }),
      (result) => {
        expect(result.status).not.toBe(0);
        expect(result.output).toContain("reported skipped/disabled tests");
        expect(result.output).toContain("skipped one");
      }
    );
  });

  it("passes a bun JUnit report with tests and no skips", () => {
    inTempDir("report.xml", passingJunit, (result) => {
      expect(result.status).toBe(0);
      expect(result.output).toContain("[assert-no-skips] OK: 3 tests");
    });
  });

  it("fails a bun JUnit report with skipped tests, naming each full name", () => {
    inTempDir("report.xml", skippedJunit, (result) => {
      expect(result.status).not.toBe(0);
      expect(result.output).toContain("reported skipped/disabled tests");
      expect(result.output).toContain("beta suite explicitly skipped (skipped)");
      expect(result.output).toContain("beta suite conditionally skipped (skipped)");
      expect(result.output).toContain("gamma suite skipped wholesale never runs a (skipped)");
      expect(result.output).toContain("gamma suite skipped wholesale never runs b (skipped)");
    });
  });

  it("fails a bun JUnit report with todo tests", () => {
    inTempDir("report.xml", todoJunit, (result) => {
      expect(result.status).not.toBe(0);
      expect(result.output).toContain("reported skipped/disabled tests");
      expect(result.output).toContain("todo without body (todo)");
      expect(result.output).toContain("todo with body (todo)");
    });
  });

  it("fails a JUnit report that collected zero testcases", () => {
    inTempDir(
      "report.xml",
      '<?xml version="1.0" encoding="UTF-8"?>\n<testsuites name="bun test" tests="0" assertions="0" failures="0" skipped="0" time="0"></testsuites>\n',
      (result) => {
        expect(result.status).not.toBe(0);
        expect(result.output).toContain("reported zero tests");
      }
    );
  });

  it("fails hard on malformed JUnit XML", () => {
    inTempDir("report.xml", '<testsuites><testsuite name="x"><testcase name="y"></testsuites>\n', (result) => {
      expect(result.status).not.toBe(0);
      expect(result.output).toContain("failed to parse");
    });
  });

  it("fails hard on a missing report path (bun writes NO junit outfile when zero tests are collected)", () => {
    const result = run(resolve(tmpdir(), "aex-no-skips-definitely-missing", "report.xml"));
    expect(result.status).not.toBe(0);
    expect(result.output).toContain("report not found");
  });
});

// `bun test -t <pattern>` marks every non-selected test <skipped/> in the junit
// report, so gate calls for -t-filtered lanes (tool-fuzz) must scope the skip
// check with --name-pattern. Semantics mirror the platform repo's copy of
// assert-no-skips.mjs exactly.
describe("assert-no-skips --name-pattern scoping", () => {
  it("fails a -t-filtered bun JUnit report when gated WITHOUT the pattern (the Wave-0 hazard)", () => {
    inTempDir("report.xml", filteredJunit, (result) => {
      expect(result.status).not.toBe(0);
      expect(result.output).toContain("reported skipped/disabled tests");
    });
  });

  it("scopes JUnit skip checks to the selected name pattern (bun -t marks the rest skipped)", () => {
    inTempDir(
      "report.xml",
      filteredJunit,
      (result) => {
        expect(result.status).toBe(0);
        expect(result.output).toContain('OK: 1 tests matching --name-pattern "adds numbers"');
      },
      ["--name-pattern", "adds numbers"]
    );
  });

  it("still fails when a selected name-pattern JUnit test is skipped", () => {
    inTempDir(
      "report.xml",
      skippedJunit,
      (result) => {
        expect(result.status).not.toBe(0);
        expect(result.output).toContain("reported skipped/disabled tests");
        expect(result.output).toContain("beta suite explicitly skipped (skipped)");
      },
      ["--name-pattern", "explicitly skipped"]
    );
  });

  it("fails loudly when the name pattern matches zero JUnit testcases", () => {
    inTempDir(
      "report.xml",
      passingJunit,
      (result) => {
        expect(result.status).not.toBe(0);
        expect(result.output).toContain('reported zero tests matching --name-pattern "no such test anywhere"');
      },
      ["--name-pattern", "no such test anywhere"]
    );
  });

  it("scopes Vitest JSON skip checks to the selected name pattern", () => {
    const report = JSON.stringify({
      numTotalTests: 2,
      numPendingTests: 1,
      testResults: [
        {
          name: "tool-fuzz.test.ts",
          assertionResults: [
            { status: "skipped", fullName: "seeded web case +0 covers web_fetch", title: "seeded web case +0" },
            { status: "passed", fullName: "seeded files case +0 covers read/write", title: "seeded files case +0" }
          ]
        }
      ]
    });
    inTempDir(
      "report.json",
      report,
      (result) => {
        expect(result.status).toBe(0);
        expect(result.output).toContain('OK: 1 tests matching --name-pattern "seeded files"');
      },
      ["--name-pattern", "seeded files"]
    );
    inTempDir(
      "report.json",
      report,
      (result) => {
        expect(result.status).not.toBe(0);
        expect(result.output).toContain("reported skipped/disabled tests");
      },
      ["--name-pattern", "seeded web"]
    );
  });

  it("rejects an invalid --name-pattern regex", () => {
    inTempDir(
      "report.xml",
      passingJunit,
      (result) => {
        expect(result.status).not.toBe(0);
        expect(result.output).toContain("invalid --name-pattern");
      },
      ["--name-pattern", "["]
    );
  });

  it("rejects --name-pattern without a value", () => {
    const result = run(["--name-pattern"]);
    expect(result.status).not.toBe(0);
    expect(result.output).toContain("--name-pattern requires a regex value");
  });

  it("rejects more than one report path (the gate proves exactly one report)", () => {
    const dir = mkdtempSync(resolve(tmpdir(), "aex-no-skips-"));
    try {
      const first = writeReport(dir, "a.xml", passingJunit);
      const second = writeReport(dir, "b.xml", passingJunit);
      const result = run([first, second]);
      expect(result.status).not.toBe(0);
      expect(result.output).toContain("usage: assert-no-skips.mjs");
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
});
