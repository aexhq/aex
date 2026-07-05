/**
 * Property fuzz for Skill NAME DERIVATION (§7.6): random unicode / mixed-case /
 * reserved-char names + frontmatter must resolve to a name matching
 * SKILL_NAME_PATTERN (and not reserved, and free of `__`) OR throw — NEVER a
 * silent bad name.
 */
import fc from "fast-check";
import { describe, expect, it } from "vitest";
import { SKILL_NAME_PATTERN, SKILL_RESERVED_NAMES, Skill } from "../../src/index.js";
import { mkdtempSync, rmSync, writeFileSync, mkdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

function skillMd(name: string | undefined): string {
  const lines = ["---"];
  if (name !== undefined) lines.push(`name: ${name}`);
  lines.push("description: A fuzzed skill.", "---", "# body", "");
  return lines.join("\n");
}

/** A name is "good" iff it matches the pattern, is not reserved, and has no `__`. */
function isGoodName(name: string): boolean {
  return SKILL_NAME_PATTERN.test(name) && !name.includes("__") && !SKILL_RESERVED_NAMES.has(name);
}

async function tryDerive(build: () => Promise<Skill>): Promise<{ ok: true; name: string } | { ok: false }> {
  try {
    const skill = await build();
    return { ok: true, name: skill.name };
  } catch {
    return { ok: false };
  }
}

// A generator that mixes valid-ish tokens with unicode, spaces, case, and the
// reserved separators/names so both accept and reject paths get exercised.
const messyName = fc.oneof(
  fc.string(),
  fc.stringMatching(/^[A-Za-z0-9 _-]{0,20}$/),
  fc.constantFrom("skills", "skill", "a__b", "UPPER", "-lead", "白菜", "café", "  ", "ok-name", "n1"),
  fc.string({ unit: "binary", maxLength: 12 })
);

describe("Skill name derivation — never a silent bad name", () => {
  it("explicit { name } either yields a good name or throws (fromContent)", async () => {
    await fc.assert(
      fc.asyncProperty(messyName, async (name) => {
        const res = await tryDerive(() => Skill.fromContent(skillMd(undefined), { name }));
        expect(res.ok ? isGoodName(res.name) : true).toBe(true);
      }),
      { numRuns: 300 }
    );
  });

  it("frontmatter name either yields a good name or throws (fromFiles)", async () => {
    await fc.assert(
      fc.asyncProperty(messyName, async (name) => {
        const res = await tryDerive(() => Skill.fromFiles({ files: { "SKILL.md": skillMd(name) } }));
        expect(res.ok ? isGoodName(res.name) : true).toBe(true);
      }),
      { numRuns: 300 }
    );
  });

  it("explicit overrides frontmatter, and the result is always good-or-throw", async () => {
    await fc.assert(
      fc.asyncProperty(messyName, messyName, async (explicit, fm) => {
        const res = await tryDerive(() => Skill.fromFiles({ name: explicit, files: { "SKILL.md": skillMd(fm) } }));
        expect(res.ok ? isGoodName(res.name) : true).toBe(true);
        // When it succeeds, the explicit name won (it is a valid good name).
        expect(res.ok ? res.name : explicit).toBe(explicit);
      }),
      { numRuns: 200 }
    );
  });

  it("fromDir basename slug fallback is good-or-throw", async () => {
    await fc.assert(
      fc.asyncProperty(fc.stringMatching(/^[A-Za-z0-9 _!.-]{0,24}$/), async (basename) => {
        const root = mkdtempSync(join(tmpdir(), "aex-skill-fuzz-"));
        // Some basenames are not creatable on the fs; skip those by falling back.
        let dir: string;
        try {
          dir = join(root, basename.length === 0 ? "x" : basename);
          mkdirSync(dir, { recursive: true });
          writeFileSync(join(dir, "SKILL.md"), skillMd(undefined), "utf8");
        } catch {
          rmSync(root, { recursive: true, force: true });
          return; // uncreatable basename — not part of the property
        }
        try {
          const res = await tryDerive(() => Skill.fromDir(dir));
          expect(res.ok ? isGoodName(res.name) : true).toBe(true);
        } finally {
          rmSync(root, { recursive: true, force: true });
        }
      }),
      { numRuns: 40 }
    );
  });
});
