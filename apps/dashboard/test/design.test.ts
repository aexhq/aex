import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const root = resolve(import.meta.dir, "..");
const tokens = readFileSync(resolve(root, "app/tokens.css"), "utf8");
const chrome = readFileSync(resolve(root, "app/app.css"), "utf8");

/**
 * The dashboard consumes the design system; it does not extend it in place.
 *
 * These checks are what make the token merge with `apps/site` a deletion of one
 * file: if no rule outside `tokens.css` names a literal colour, and every custom
 * property a rule references is declared in `tokens.css`, then swapping that file
 * for the site's cannot leave a dangling reference or an unthemed value behind.
 */

function declared(source: string): Set<string> {
  return new Set([...source.matchAll(/(--aex-[a-z0-9-]+)\s*:/g)].map((match) => match[1]!));
}

function referenced(source: string): Set<string> {
  return new Set([...source.matchAll(/var\((--[a-z0-9-]+)/g)].map((match) => match[1]!));
}

test("every custom property the chrome references is declared by the token contract", () => {
  const names = declared(tokens);
  expect(names.size).toBeGreaterThan(30);
  for (const name of referenced(chrome)) {
    expect({ name, declared: names.has(name) }).toEqual({ name, declared: true });
  }
});

test("every token the chrome references is namespaced to the shared contract", () => {
  for (const name of referenced(chrome)) {
    expect({ name, namespaced: name.startsWith("--aex-") }).toEqual({ name, namespaced: true });
  }
});

test("no rule outside the token file names a literal colour", () => {
  const withoutComments = chrome.replace(/\/\*[\s\S]*?\*\//g, "");
  expect(withoutComments).not.toMatch(/#[0-9a-fA-F]{3,8}\b/);
  expect(withoutComments).not.toMatch(/\b(?:rgb|rgba|hsl|hsla|oklch|color-mix)\(/);
});

test("both modes are selectable: by the operating system and by an explicit attribute", () => {
  expect(tokens).toContain("@media (prefers-color-scheme: dark)");
  expect(tokens).toContain(':root[data-theme="dark"]');
  expect(tokens).toContain(':root[data-theme="light"]');
  expect(tokens).toContain("color-scheme: light");
  expect(tokens).toContain("color-scheme: dark");
});

test("reduced motion is honoured and the only animation is inside that guard", () => {
  expect(chrome).toContain("@media (prefers-reduced-motion: reduce)");
  const guard = /@media \(prefers-reduced-motion: reduce\)\s*\{[\s\S]*?animation-duration: 0\.01ms/;
  expect(chrome).toMatch(guard);
  expect(tokens).toContain("@media (prefers-reduced-motion: reduce)");
});

test("wide content scrolls inside its own container and the body never scrolls sideways", () => {
  expect(chrome).toMatch(/html\s*\{[^}]*overflow-x:\s*hidden/);
  expect(chrome).toMatch(/\.scroller\s*\{[^}]*overflow-x:\s*auto/);
});

test("focus is always visible and never removed", () => {
  expect(chrome).toMatch(/:focus-visible\s*\{[^}]*outline:\s*2px solid var\(--aex-color-focus\)/);
  expect(chrome).not.toMatch(/outline:\s*(?:none|0)\b/);
});

test("the status palette is the four reserved roles and nothing else", () => {
  const statuses = [...tokens.matchAll(/(--aex-status-[a-z-]+)\s*:/g)].map((match) => match[1]!);
  expect(statuses.sort()).toEqual([
    "--aex-status-critical",
    "--aex-status-good",
    "--aex-status-serious",
    "--aex-status-warning",
  ]);
});
