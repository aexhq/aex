// Local ESLint plugin for antpath-specific test-quality rules.
//
// Background: Phase 2 of the test-bar remediation. The custom rules below
// mechanically block the assertion-weakening anti-patterns that Phase 1
// fixed by hand, so an AI author (or a tired human) cannot quietly
// re-introduce them. Update this local rule deliberately if that happens.
//
// The plugin is ESM and ships as a single file — no build step. Add new
// rules by writing a `meta`/`create` pair and registering it under `rules`
// at the bottom.

/**
 * Walk up from `node` looking for an `it(...)`, `test(...)`, or
 * `it.each(...)`/`test.each(...)` CallExpression body. Used by every rule
 * here to scope its checks to test bodies only — production code can still
 * use these patterns (discriminated-union narrowing, optional-chain skips,
 * etc.) where they're legitimate.
 */
function isInsideTestBody(node) {
  let cur = node.parent;
  while (cur) {
    if (cur.type === "CallExpression") {
      const callee = cur.callee;
      // `it("...", () => { ... })` / `test("...", () => { ... })`
      if (callee && callee.type === "Identifier" && (callee.name === "it" || callee.name === "test")) {
        return true;
      }
      // `it.each(CELLS)("...", () => { ... })` / `it.skipIf(...)("...", () => { ... })`
      // — the outer call's callee is a MemberExpression off `it`/`test`.
      if (callee && callee.type === "MemberExpression" && callee.object && callee.object.type === "Identifier" && (callee.object.name === "it" || callee.object.name === "test")) {
        return true;
      }
      // `it.each(CELLS)("...", () => { ... })` — `it.each(CELLS)` is itself
      // a CallExpression returning a function, so we may be inside the
      // returned function's call. Climb past it.
    }
    cur = cur.parent;
  }
  return false;
}

/**
 * Recursively check whether a node's subtree contains a `expect(...)` call.
 * Used to decide whether an `if` body is wrapping an assertion (the bad case)
 * vs. unrelated control flow.
 */
function containsExpectCall(node) {
  if (!node || typeof node !== "object") return false;
  if (node.type === "CallExpression" && node.callee && node.callee.type === "Identifier" && node.callee.name === "expect") {
    return true;
  }
  // expect(x).toBe(y) chains: the outer `.toBe(y)` is a CallExpression whose
  // callee is a MemberExpression whose object is `expect(x)`.
  if (node.type === "CallExpression" && node.callee && node.callee.type === "MemberExpression") {
    let cur = node.callee.object;
    while (cur && cur.type === "CallExpression") {
      if (cur.callee && cur.callee.type === "Identifier" && cur.callee.name === "expect") return true;
      cur = cur.callee && cur.callee.type === "MemberExpression" ? cur.callee.object : null;
    }
  }
  // Walk all enumerable child nodes.
  for (const key of Object.keys(node)) {
    if (key === "parent" || key === "loc" || key === "range") continue;
    const child = node[key];
    if (Array.isArray(child)) {
      for (const c of child) if (containsExpectCall(c)) return true;
    } else if (child && typeof child === "object" && typeof child.type === "string") {
      if (containsExpectCall(child)) return true;
    }
  }
  return false;
}

/**
 * Pretty-print the test expression (for error messages). Falls back to
 * `<expr>` when the shape isn't a simple identifier or comparison.
 */
function snippetOf(node) {
  if (!node) return "<expr>";
  if (node.type === "Identifier") return node.name;
  if (node.type === "MemberExpression") {
    const obj = snippetOf(node.object);
    const prop = node.computed ? `[${snippetOf(node.property)}]` : `.${node.property && node.property.name ? node.property.name : "<prop>"}`;
    return obj + prop;
  }
  if (node.type === "BinaryExpression") return `${snippetOf(node.left)} ${node.operator} ${snippetOf(node.right)}`;
  if (node.type === "LogicalExpression") return `${snippetOf(node.left)} ${node.operator} ${snippetOf(node.right)}`;
  if (node.type === "Literal") return JSON.stringify(node.value);
  return "<expr>";
}

// ---------------------------------------------------------------------------
// Rule: no-undefined-skip-expect
//
// Blocks the exact anti-pattern Phase 1 spent its time fixing:
//   if (x !== undefined) { expect(x)... }
//   if (x != null)       { expect(x)... }
//   if (typeof x !== "undefined") { expect(x)... }
//
// Why this is poisonous: the assertion ONLY runs when `x` is defined. If a
// regression makes `x` go missing entirely, the test passes silently — it
// pretends to assert but doesn't. Real assertions should either:
//   (a) require the field unconditionally (`expect(x).toBe(...)`), or
//   (b) be paired with a positive presence assertion first
//       (`expect(x).toBeDefined()` then narrow).
// ---------------------------------------------------------------------------
const noUndefinedSkipExpect = {
  meta: {
    type: "problem",
    docs: {
      description:
        "Disallow `if (x !== undefined) { expect(...) }` patterns — they pass silently when x is absent.",
      recommended: true
    },
    schema: [],
    messages: {
      undefinedSkipExpect:
        "Assertion is skipped when `{{name}}` is undefined. This is a silent-skip anti-pattern: a regression that drops `{{name}}` would pass the test. Assert unconditionally, or pair `expect({{name}}).toBeDefined()` with a follow-up narrow."
    }
  },
  create(context) {
    return {
      IfStatement(node) {
        if (!isInsideTestBody(node)) return;
        const test = node.test;
        if (!test) return;
        // Match: `x !== undefined`, `x != null`, `x != undefined`
        const isUndefinedCheck =
          test.type === "BinaryExpression" &&
          (test.operator === "!==" || test.operator === "!=") &&
          ((test.right && test.right.type === "Identifier" && test.right.name === "undefined") ||
            (test.right && test.right.type === "Literal" && test.right.value === null) ||
            (test.left && test.left.type === "Identifier" && test.left.name === "undefined") ||
            (test.left && test.left.type === "Literal" && test.left.value === null));
        // Match: `typeof x !== "undefined"`
        const isTypeofCheck =
          test.type === "BinaryExpression" &&
          (test.operator === "!==" || test.operator === "!=") &&
          ((test.left && test.left.type === "UnaryExpression" && test.left.operator === "typeof") ||
            (test.right && test.right.type === "UnaryExpression" && test.right.operator === "typeof"));
        // Match: `x !== undefined && x !== "complete"` — the throw-flavoured
        // version is also a silent-skip (when x is undefined, the && short-
        // circuits and the throw never fires).
        const isUndefinedThenCompare =
          test.type === "LogicalExpression" &&
          test.operator === "&&" &&
          test.left && test.left.type === "BinaryExpression" &&
          (test.left.operator === "!==" || test.left.operator === "!=") &&
          ((test.left.right && test.left.right.type === "Identifier" && test.left.right.name === "undefined") ||
            (test.left.right && test.left.right.type === "Literal" && test.left.right.value === null));
        if (!isUndefinedCheck && !isTypeofCheck && !isUndefinedThenCompare) return;
        // Body must wrap an assertion (expect call or throw) — otherwise
        // this is just control flow (e.g. early-return guard), not an
        // assertion skip.
        const wrapsExpect = containsExpectCall(node.consequent);
        const wrapsThrow = bodyContainsThrow(node.consequent);
        if (!wrapsExpect && !wrapsThrow) return;
        context.report({
          node,
          messageId: "undefinedSkipExpect",
          data: { name: snippetOf(test) }
        });
      }
    };
  }
};

function bodyContainsThrow(node) {
  if (!node || typeof node !== "object") return false;
  if (node.type === "ThrowStatement") return true;
  for (const key of Object.keys(node)) {
    if (key === "parent" || key === "loc" || key === "range") continue;
    const child = node[key];
    if (Array.isArray(child)) {
      for (const c of child) if (bodyContainsThrow(c)) return true;
    } else if (child && typeof child === "object" && typeof child.type === "string") {
      if (bodyContainsThrow(child)) return true;
    }
  }
  return false;
}

// ---------------------------------------------------------------------------
// Rule: no-disabled-tests
//
// Blocks `it.skip`, `test.skip`, `describe.skip`, `xit`, `xdescribe`. The
// platform-conditional `it.skipIf(...)` is allowed (it gates on a runtime
// signal, not "this test is broken, leave it for later").
// ---------------------------------------------------------------------------
const noDisabledTests = {
  meta: {
    type: "problem",
    docs: { description: "Disallow disabled/skipped test markers.", recommended: true },
    schema: [],
    messages: {
      disabled:
        "`{{name}}` disables a test. Either fix it, delete it, or use the platform-conditional `skipIf(...)` (which gates on runtime conditions, not on the test being broken)."
    }
  },
  create(context) {
    return {
      CallExpression(node) {
        const callee = node.callee;
        if (!callee) return;
        // it.skip(...) / test.skip(...) / describe.skip(...)
        if (
          callee.type === "MemberExpression" &&
          callee.object && callee.object.type === "Identifier" &&
          (callee.object.name === "it" || callee.object.name === "test" || callee.object.name === "describe") &&
          callee.property && callee.property.type === "Identifier" &&
          (callee.property.name === "skip" || callee.property.name === "todo")
        ) {
          context.report({ node, messageId: "disabled", data: { name: `${callee.object.name}.${callee.property.name}` } });
        }
        // xit(...) / xtest(...) / xdescribe(...)
        if (callee.type === "Identifier" && (callee.name === "xit" || callee.name === "xtest" || callee.name === "xdescribe")) {
          context.report({ node, messageId: "disabled", data: { name: callee.name } });
        }
      }
    };
  }
};

// ---------------------------------------------------------------------------
// Rule: no-focused-tests
//
// Blocks `it.only`, `test.only`, `describe.only`, `fit`, `fdescribe`. A
// focused test makes the whole suite a no-op in CI; banning it at lint
// time prevents accidental commits.
// ---------------------------------------------------------------------------
const noFocusedTests = {
  meta: {
    type: "problem",
    docs: { description: "Disallow focused/exclusive test markers.", recommended: true },
    schema: [],
    messages: {
      focused:
        "`{{name}}` focuses a single test and silently skips every other test in the suite. Remove before commit."
    }
  },
  create(context) {
    return {
      CallExpression(node) {
        const callee = node.callee;
        if (!callee) return;
        if (
          callee.type === "MemberExpression" &&
          callee.object && callee.object.type === "Identifier" &&
          (callee.object.name === "it" || callee.object.name === "test" || callee.object.name === "describe") &&
          callee.property && callee.property.type === "Identifier" &&
          callee.property.name === "only"
        ) {
          context.report({ node, messageId: "focused", data: { name: `${callee.object.name}.only` } });
        }
        if (callee.type === "Identifier" && (callee.name === "fit" || callee.name === "fdescribe")) {
          context.report({ node, messageId: "focused", data: { name: callee.name } });
        }
      }
    };
  }
};

// ---------------------------------------------------------------------------
// Rule: no-conditional-expect
//
// Broader cousin of `no-undefined-skip-expect`: ANY `if` in a test body
// whose consequent wraps an `expect(...)` call is reported — UNLESS the
// immediately preceding statement is a positive assertion on the same
// identifier the `if` is testing (the discriminated-union narrowing
// pattern used by SSRF/policy tests).
//
// This is intentionally noisy — every flagged site is either a real bug
// or a legitimate case that needs an explicit `// eslint-disable-next-line`
// + a reviewer comment. Making the pattern visible IS the point.
// ---------------------------------------------------------------------------
const noConditionalExpect = {
  meta: {
    type: "problem",
    docs: {
      description:
        "Disallow `if (...) { expect(...) }` in test bodies, unless narrowing a discriminated union proven by the immediately preceding assertion."
    },
    schema: [],
    messages: {
      conditional:
        "Conditional assertion — the `expect(...)` is skipped when the `if` is falsy. If you're narrowing a discriminated union (e.g. `expect(result.ok).toBe(true)` then `if (result.ok)`), the preceding assertion must be on the SAME identifier the `if` is testing. Otherwise, assert unconditionally."
    }
  },
  create(context) {
    return {
      IfStatement(node) {
        if (!isInsideTestBody(node)) return;
        if (!containsExpectCall(node.consequent)) return;
        // Carve-out: if the immediately preceding sibling statement is
        // `expect(<root of test condition>)...`, this is a legitimate
        // narrow.
        const conditionRoot = rootIdentifierOf(node.test);
        if (!conditionRoot) {
          context.report({ node, messageId: "conditional" });
          return;
        }
        const parent = node.parent;
        if (parent && parent.type === "BlockStatement" && Array.isArray(parent.body)) {
          const idx = parent.body.indexOf(node);
          for (let i = idx - 1; i >= 0; i--) {
            const prev = parent.body[i];
            if (!prev) break;
            // Skip over plain assignment/declaration statements (the
            // narrow may be preceded by a const that binds the value).
            if (prev.type === "VariableDeclaration") continue;
            if (prev.type === "ExpressionStatement" && prev.expression && isExpectOnRoot(prev.expression, conditionRoot)) {
              return; // legitimate narrow
            }
            break;
          }
        }
        context.report({ node, messageId: "conditional" });
      }
    };
  }
};

/**
 * Extract the root identifier name from an `if` test:
 *   `result.ok`     → "result"
 *   `!result.ok`    → "result"
 *   `result`        → "result"
 *   `terminal`      → "terminal"
 * Returns null for compound expressions we can't reduce.
 */
function rootIdentifierOf(node) {
  if (!node) return null;
  if (node.type === "Identifier") return node.name;
  if (node.type === "MemberExpression") return rootIdentifierOf(node.object);
  if (node.type === "UnaryExpression") return rootIdentifierOf(node.argument);
  return null;
}

/**
 * True if `expr` is `expect(<x>...).<matcher>(...)` where `<x>` starts with
 * the given identifier root. Used by the no-conditional-expect carve-out to
 * recognise the SSRF discriminated-union narrowing pattern:
 *   expect(result.ok).toBe(true);  // root="result"
 *   if (result.ok) { expect(result.value).toBe(...); }
 */
function isExpectOnRoot(expr, rootName) {
  // Walk the matcher chain back to the seed `expect(...)` call.
  let cur = expr;
  while (cur && cur.type === "CallExpression") {
    if (cur.callee && cur.callee.type === "Identifier" && cur.callee.name === "expect") {
      const arg = cur.arguments && cur.arguments[0];
      return arg ? rootIdentifierOf(arg) === rootName : false;
    }
    cur = cur.callee && cur.callee.type === "MemberExpression" ? cur.callee.object : null;
  }
  return false;
}

export default {
  rules: {
    "no-undefined-skip-expect": noUndefinedSkipExpect,
    "no-disabled-tests": noDisabledTests,
    "no-focused-tests": noFocusedTests,
    "no-conditional-expect": noConditionalExpect
  }
};
