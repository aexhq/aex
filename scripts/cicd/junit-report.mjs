/**
 * JUnit XML test-report parsing for the release-gate scripts.
 *
 * Two dialects are understood, both verified against REAL reporter output:
 *   - bun test >=1.3.14 (`bun test --reporter=junit --reporter-outfile=...`):
 *     <testsuites> wraps one <testsuite name="<file>" file="<file>"> per test
 *     file; describe blocks nest further <testsuite> elements; skips are a
 *     `<skipped />` child of the <testcase>, todos are `<skipped message="TODO" />`,
 *     and every suite level repeats aggregate `tests`/`skipped` count attributes.
 *     bun writes NO outfile at all when zero tests are collected — a missing
 *     report file must therefore stay a hard failure at the call site.
 *   - node --test (`--test-reporter=junit`): bare <testcase> elements directly
 *     under <testsuites>, subtests as nested <testsuite> elements, skips as
 *     `<skipped type="skipped"/>` and todos as `<skipped type="todo"/>`, with
 *     aggregate counters only present in XML comments.
 *
 * The parser is a deliberately small, strict, dependency-free reader of exactly
 * this subset (elements, attributes, comments, CDATA, entities). Anything
 * structurally malformed throws — an unreadable report can never prove
 * "no skips", so callers treat a throw as a gate failure, not a pass.
 *
 * This file is intentionally byte-identical between the aex and platform
 * repositories (each release gate carries its own copy next to its
 * assert-no-skips.mjs). Change both together.
 */

const ENTITIES = { amp: "&", lt: "<", gt: ">", quot: '"', apos: "'" };

/**
 * Content-based shape sniff: JUnit reports are XML documents, the other gate
 * report shapes (Vitest jest-style, Playwright) are JSON. Keyed on content —
 * never on file extension — so misnamed reports cannot dodge the right parser.
 * @param {string} text
 * @returns {boolean}
 */
export function isJunitReportText(text) {
  return typeof text === "string" && text.replace(/^\uFEFF/, "").trimStart().startsWith("<");
}

/**
 * @param {string} value
 * @returns {string}
 */
function decodeEntities(value) {
  return value.replace(/&(#[xX]?[0-9a-fA-F]+|[a-zA-Z]+);/g, (match, body) => {
    if (body.startsWith("#x") || body.startsWith("#X")) {
      return String.fromCodePoint(Number.parseInt(body.slice(2), 16));
    }
    if (body.startsWith("#")) return String.fromCodePoint(Number.parseInt(body.slice(1), 10));
    return Object.hasOwn(ENTITIES, body) ? ENTITIES[body] : match;
  });
}

/**
 * @typedef {{ tag: string; attributes: Record<string, string>; children: JunitElement[]; text: string }} JunitElement
 */

/**
 * Parse the JUnit XML subset into an element tree. Throws on malformed input
 * (mismatched/unclosed tags, no root element) — strict by design.
 * @param {string} text
 * @returns {JunitElement} synthetic document root whose children are the top-level elements
 */
export function parseJunitDocument(text) {
  const source = text.replace(/^\uFEFF/, "");
  /** @type {JunitElement} */
  const documentRoot = { tag: "#document", attributes: {}, children: [], text: "" };
  const stack = [documentRoot];
  const token =
    /<!\[CDATA\[([\s\S]*?)\]\]>|<!--[\s\S]*?-->|<\?[\s\S]*?\?>|<!DOCTYPE[^>]*>|<\/\s*([^\s>]+)\s*>|<([^\s/>!?]+)((?:"[^"]*"|'[^']*'|[^"'>])*?)(\/?)>|([^<]+)/g;
  let matchedLength = 0;
  for (const match of source.matchAll(token)) {
    matchedLength += match[0].length;
    const [, cdata, closeTag, openTag, rawAttributes, selfClosing, textRun] = match;
    const current = stack[stack.length - 1];
    if (cdata !== undefined) {
      current.text += cdata;
    } else if (closeTag !== undefined) {
      if (stack.length < 2 || current.tag !== closeTag) {
        throw new Error(`malformed JUnit XML: unexpected closing tag </${closeTag}>`);
      }
      stack.pop();
    } else if (openTag !== undefined) {
      /** @type {JunitElement} */
      const element = { tag: openTag, attributes: {}, children: [], text: "" };
      for (const attribute of rawAttributes.matchAll(/([^\s=/]+)\s*=\s*(?:"([^"]*)"|'([^']*)')/g)) {
        element.attributes[attribute[1]] = decodeEntities(attribute[2] ?? attribute[3] ?? "");
      }
      current.children.push(element);
      if (!selfClosing) stack.push(element);
    } else if (textRun !== undefined) {
      current.text += decodeEntities(textRun);
    }
  }
  if (matchedLength !== source.length) {
    throw new Error("malformed JUnit XML: unparseable markup");
  }
  if (stack.length !== 1) {
    throw new Error(`malformed JUnit XML: unclosed <${stack[stack.length - 1].tag}>`);
  }
  if (documentRoot.children.length === 0) {
    throw new Error("malformed JUnit XML: no root element");
  }
  return documentRoot;
}

/**
 * @typedef {{ fullName: string; file: string; status: "passed" | "failed" | "skipped" | "todo"; message: string }} JunitTestcase
 */

/**
 * Flatten every <testcase> with its jest/vitest-convention full name: ancestor
 * suite titles + testcase name, space-joined. bun's per-file wrapper suite
 * (name === file attribute) and the root <testsuites> contribute NO title —
 * only describe-block suites (and node subtest parents) do.
 * @param {string} text
 * @returns {JunitTestcase[]}
 */
export function listJunitTestcases(text) {
  const documentRoot = parseJunitDocument(text);
  /** @type {JunitTestcase[]} */
  const testcases = [];
  /** @param {JunitElement} element @param {string[]} titles @param {string | undefined} file */
  const visit = (element, titles, file) => {
    for (const child of element.children) {
      const childFile = child.attributes.file ?? file;
      if (child.tag === "testcase") {
        const marker = child.children.find((node) => node.tag === "skipped");
        const failure = child.children.find((node) => node.tag === "failure" || node.tag === "error");
        const status = marker
          ? /^todo$/i.test(marker.attributes.type ?? "") || /^todo$/i.test(marker.attributes.message ?? "")
            ? "todo"
            : "skipped"
          : failure
            ? "failed"
            : "passed";
        testcases.push({
          fullName: [...titles, child.attributes.name ?? "(unnamed)"].join(" "),
          file: childFile ?? "(unknown file)",
          status,
          message: failure
            ? [failure.attributes.type, failure.attributes.message, failure.text.trim()].filter(Boolean).join(": ")
            : ""
        });
      } else if (child.tag === "testsuite") {
        const title = child.attributes.name;
        const isFileSuite = !title || title === child.attributes.file;
        visit(child, isFileSuite ? titles : [...titles, title], childFile);
      } else if (child.tag === "testsuites") {
        visit(child, titles, childFile);
      }
    }
  };
  visit(documentRoot, [], undefined);
  return testcases;
}

/**
 * Gate view of a JUnit report: total testcases and the skipped/todo ones,
 * optionally scoped to full names matching `namePattern` (mirroring the
 * Vitest-JSON branch semantics — `bun test -t` marks every non-selected test
 * `<skipped/>`, so an unscoped read of a filtered run would false-fail).
 * @param {string} text
 * @param {{ namePattern?: RegExp }} [options]
 * @returns {{ total: number; skipped: { file: string; name: string; status: string }[] }}
 */
export function collectJunitTests(text, options = {}) {
  const namePattern = options.namePattern;
  const testcases = listJunitTestcases(text);
  let total = 0;
  /** @type {{ file: string; name: string; status: string }[]} */
  const skipped = [];
  let itemizedSkips = 0;
  for (const testcase of testcases) {
    if (testcase.status === "skipped" || testcase.status === "todo") itemizedSkips += 1;
    if (namePattern && !namePattern.test(testcase.fullName)) continue;
    total += 1;
    if (testcase.status === "skipped" || testcase.status === "todo") {
      skipped.push({ file: testcase.file, name: testcase.fullName, status: testcase.status });
    }
  }

  // Cross-check the root-level aggregate counters (defense in depth, mirroring
  // the Vitest/Playwright branches): if the reporter DECLARES more skips than
  // we could itemize from <testcase> children, surface that rather than pass.
  // Root level only — bun repeats `skipped` at every suite nesting level, so
  // summing all suites would double-count.
  const declaredRoot = parseJunitDocument(text).children.find(
    (element) => element.tag === "testsuites" || element.tag === "testsuite"
  );
  const declaredSkips =
    Number.parseInt(declaredRoot?.attributes.skipped ?? "", 10) +
    (Number.parseInt(declaredRoot?.attributes.disabled ?? "0", 10) || 0);
  if (!namePattern && Number.isFinite(declaredSkips) && declaredSkips > itemizedSkips) {
    skipped.push({
      file: "(report)",
      name: `(suite counters declare ${declaredSkips} skipped test(s) this report could not be itemized)`,
      status: "skipped"
    });
  }

  return { total, skipped };
}

/**
 * Diagnostics view of a JUnit report (for CI failure evidence): aggregate
 * counts plus itemized failures.
 * @param {string} text
 * @returns {{
 *   totalTests: number;
 *   passedTests: number;
 *   failedTests: number;
 *   skippedTests: number;
 *   failed: { file: string; name: string; message: string }[];
 * }}
 */
export function summarizeJunitReport(text) {
  const testcases = listJunitTestcases(text);
  const failed = testcases
    .filter((testcase) => testcase.status === "failed")
    .map((testcase) => ({ file: testcase.file, name: testcase.fullName, message: testcase.message }));
  const skippedTests = testcases.filter(
    (testcase) => testcase.status === "skipped" || testcase.status === "todo"
  ).length;
  return {
    totalTests: testcases.length,
    passedTests: testcases.length - failed.length - skippedTests,
    failedTests: failed.length,
    skippedTests,
    failed
  };
}
