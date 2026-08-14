import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

/**
 * The design-system gate.
 *
 * The tokens are shared with `apps/dashboard`, so a colour edit here changes
 * two products. Contrast is therefore checked mechanically rather than by eye:
 * every foreground/background pair the components actually compose is asserted
 * against the WCAG 2.2 AA threshold, in both themes, and the light and dark
 * palettes are asserted to define the same token names so neither app can hit
 * an undefined variable in one theme only.
 */
const DESIGN_ROOT = resolve(import.meta.dir, "../design");
const tokensCss = readFileSync(resolve(DESIGN_ROOT, "tokens.css"), "utf8");
const baseCss = readFileSync(resolve(DESIGN_ROOT, "base.css"), "utf8");
const componentsCss = readFileSync(resolve(DESIGN_ROOT, "components.css"), "utf8");
const siteCss = readFileSync(resolve(import.meta.dir, "../app/site.css"), "utf8");

/** WCAG 2.2: 4.5:1 for body text, 3:1 for large text and non-text UI. */
const AA_TEXT = 4.5;
const AA_NON_TEXT = 3;

const SCOPE_MARKERS: readonly (readonly [string, string])[] = [
  ["light", ":root {"],
  ["dark", "@media (prefers-color-scheme: dark)"],
  ["light-attribute", ':root[data-theme="light"]'],
  ["dark-attribute", ':root[data-theme="dark"]']
];

/** `text on background` pairs, with the threshold each one has to clear. */
const CONTRAST_PAIRS: readonly (readonly [string, string, number])[] = [
  ["text", "canvas", AA_TEXT],
  ["text", "surface", AA_TEXT],
  ["text", "surface-sunken", AA_TEXT],
  ["text", "accent-surface", AA_TEXT],
  ["text-muted", "canvas", AA_TEXT],
  ["text-muted", "surface", AA_TEXT],
  ["text-muted", "surface-sunken", AA_TEXT],
  ["text-faint", "canvas", AA_TEXT],
  ["text-faint", "surface", AA_TEXT],
  ["accent", "canvas", AA_TEXT],
  ["accent", "surface", AA_TEXT],
  ["accent", "surface-sunken", AA_TEXT],
  ["accent", "accent-surface", AA_TEXT],
  ["accent-hover", "surface", AA_TEXT],
  ["accent-contrast", "accent", AA_TEXT],
  ["accent-contrast", "accent-hover", AA_TEXT],
  ["focus", "canvas", AA_NON_TEXT],
  ["focus", "surface", AA_NON_TEXT],
  ["border-strong", "canvas", AA_NON_TEXT],
  ["border-strong", "surface", AA_NON_TEXT]
];

const palettes = readPalettes(tokensCss);

describe("design tokens", () => {
  test("defines all four theme scopes", () => {
    expect([...palettes.keys()].sort()).toEqual(["dark", "dark-attribute", "light", "light-attribute"]);
  });

  test("keeps the media-query and attribute palettes identical", () => {
    // A toggle that disagrees with the system preference is a second palette
    // to maintain, and the one that drifts is always the unused one.
    expect(Object.fromEntries(palettes.get("light-attribute") as Map<string, string>)).toEqual(
      Object.fromEntries(palettes.get("light") as Map<string, string>)
    );
    expect(Object.fromEntries(palettes.get("dark-attribute") as Map<string, string>)).toEqual(
      Object.fromEntries(palettes.get("dark") as Map<string, string>)
    );
  });

  test("defines the same colour token names in light and dark", () => {
    expect([...(palettes.get("dark") as Map<string, string>).keys()].sort()).toEqual(
      [...(palettes.get("light") as Map<string, string>).keys()].sort()
    );
  });

  test("clears the AA contrast threshold for every composed pair, in both themes", () => {
    const failures: string[] = [];
    for (const theme of ["light", "dark"] as const) {
      const palette = palettes.get(theme) as Map<string, string>;
      for (const [foreground, background, threshold] of CONTRAST_PAIRS) {
        const ratio = contrast(require_(palette, foreground), require_(palette, background));
        if (ratio < threshold) {
          failures.push(`${theme}: ${foreground} on ${background} is ${ratio.toFixed(2)}:1, needs ${threshold}:1`);
        }
      }
    }

    expect(failures).toEqual([]);
  });

  test("exposes the full token vocabulary the dashboard has to reuse", () => {
    const required = [
      "--aex-color-canvas",
      "--aex-color-surface",
      "--aex-color-text",
      "--aex-color-accent",
      "--aex-color-focus",
      "--aex-font-sans",
      "--aex-font-mono",
      "--aex-text-base",
      "--aex-text-display",
      "--aex-leading-normal",
      "--aex-space-4",
      "--aex-radius-md",
      "--aex-shadow-md",
      "--aex-width-page",
      "--aex-duration-fast",
      "--aex-ease"
    ];

    expect(required.filter((token) => !tokensCss.includes(`${token}:`))).toEqual([]);
  });

  test("honours prefers-reduced-motion in both the tokens and the element base", () => {
    expect(tokensCss).toContain("@media (prefers-reduced-motion: reduce)");
    expect(baseCss).toContain("@media (prefers-reduced-motion: reduce)");
  });

  test("keeps a visible focus indicator and a skip link", () => {
    expect(baseCss).toContain(":focus-visible");
    expect(componentsCss).toContain(".aex-skip");
  });

  test("collapses every responsive grid instead of overflowing a narrow viewport", () => {
    // `minmax(19rem, 1fr)` overflows a 320px viewport; `minmax(min(19rem,
    // 100%), 1fr)` collapses to one column. A page with no responsive minmax
    // grid also satisfies the invariant.
    const tracks = [...siteCss.matchAll(/minmax\(([^)]*\)?[^)]*)\)/g)].map((match) => match[1] as string);

    expect(tracks.filter((track) => !track.startsWith("min("))).toEqual([]);
  });
});

function require_(palette: Map<string, string>, name: string): string {
  const value = palette.get(name);
  if (value === undefined) throw new Error(`token --aex-color-${name} is not defined`);
  return value;
}

/** Reads `--aex-color-*` declarations, grouped by the theme scope they sit in. */
function readPalettes(css: string): Map<string, Map<string, string>> {
  const bounds = SCOPE_MARKERS.map(([name, marker]) => {
    const start = css.indexOf(marker);
    if (start === -1) throw new Error(`tokens.css has no "${marker}" scope`);
    return { name, start };
  }).sort((left, right) => left.start - right.start);

  const palettes = new Map<string, Map<string, string>>();
  for (const [position, scope] of bounds.entries()) {
    const end = bounds[position + 1]?.start ?? css.length;
    const palette = new Map<string, string>();
    for (const match of css.slice(scope.start, end).matchAll(/--aex-color-([a-z-]+):\s*(#[0-9a-f]{6})\s*;/g)) {
      palette.set(match[1] as string, match[2] as string);
    }
    palettes.set(scope.name, palette);
  }
  return palettes;
}

function contrast(foreground: string, background: string): number {
  const light = relativeLuminance(foreground);
  const dark = relativeLuminance(background);
  const [high, low] = light > dark ? [light, dark] : [dark, light];
  return (high + 0.05) / (low + 0.05);
}

function relativeLuminance(hex: string): number {
  const channels = [1, 3, 5].map((offset) => {
    const value = Number.parseInt(hex.slice(offset, offset + 2), 16) / 255;
    return value <= 0.040_45 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * (channels[0] as number) + 0.7152 * (channels[1] as number) + 0.0722 * (channels[2] as number);
}
