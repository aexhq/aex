// Self-test fixture for the aex ESLint rules. Lives next to the plugin
// so it ships with the rules. NOT a Vitest test — sessions as a fixture-style
// lint smoke (`bun run lint:tests` will fail with EXACTLY the
// expected aex/* violations on this file). The verification script
// at the bottom of this comment block sessions it programmatically; manual
// re-introduction here serves the same purpose for ad-hoc verification.
//
// To use: `bun run lint:tests:verify` — see package.json.
//
// Each block below intentionally trips one rule. If a rule stops
// catching the pattern (e.g. after a refactor), the verify command's
// expected-vs-actual diff surfaces the regression.

declare const it: (name: string, fn: () => void) => void;
declare const expect: (x: unknown) => { toBe: (v: unknown) => void; toBeDefined: () => void };

// PATTERN 1 — no-undefined-skip-expect: silent skip when field absent.
it("VERIFY-PATTERN-1", () => {
  const x: { reason?: string } = {};
  const r = x.reason;
  if (r !== undefined) {
    expect(r).toBe("complete");
  }
});

// PATTERN 2 — no-conditional-expect: skip assertion when truthy check is false.
it("VERIFY-PATTERN-2", () => {
  const events: Array<{ kind: string }> = [];
  const terminal = events.find((e) => e.kind === "runtime_terminal");
  if (terminal) {
    expect(terminal.kind).toBe("runtime_terminal");
  }
});

// PATTERN 3 — no-disabled-tests
it.skip("VERIFY-PATTERN-3", () => {
  expect(1).toBe(1);
});

it.skipIf(true)("VERIFY-PATTERN-4", () => {
  expect(1).toBe(1);
});

// PATTERN 5 — no-focused-tests
it.only("VERIFY-PATTERN-5", () => {
  expect(1).toBe(1);
});

// PATTERN 6 — no-disabled-tests: bun-only `.if` alias (run-only-if, bun's
// spelling of vitest's `.runIf`).
it.if(true)("VERIFY-PATTERN-6", () => {
  expect(1).toBe(1);
});

// PATTERN 7 — no-disabled-tests: bun-only `.todoIf` alias (conditional
// `.todo`).
it.todoIf(true)("VERIFY-PATTERN-7", () => {
  expect(1).toBe(1);
});

// ALLOWED — bun's `.failing` ASSERTS the test fails (it runs the body and
// errors if it passes), so it is an assertion, not a skip. It must NOT trip
// any rule; the verify script's unexpected-rule check catches a regression
// that starts flagging it.
it.failing("VERIFY-ALLOWED-FAILING", () => {
  expect(1).toBe(2);
});
