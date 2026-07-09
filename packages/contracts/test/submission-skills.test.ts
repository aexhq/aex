/**
 * Contract tests for the by-name SKILLS surface:
 *   - `parseSkills` (name-only refs; pattern / `__` / reserved / dedup / bound),
 *   - the `submission.tools` redirect for a stray `kind:"skill"` entry,
 *   - `resolvedSkills` rejected on ingress but accepted under a trusted re-parse,
 *   - idempotency: bytes-differ-same-name hashes EQUAL; name-differ hashes DIFFER
 *     (the public submission carries name-only refs, so bytes never enter it),
 *   - the injected `skills` meta-tool contract (name kept OUT of BUILTIN_TOOL_NAMES).
 */
import { describe, expect, it } from "vitest";
import fc from "fast-check";
import {
  BUILTIN_TOOL_NAMES,
  Models,
  SKILLS_MAX,
  SKILLS_TOOL_DEFINITION,
  SKILLS_TOOL_NAME,
  parseSessionSubmissionRequest,
  parseSkills,
  sha256
} from "../src/index.js";

function baseRequest() {
  return {
    workspaceId: "workspace-1",
    idempotencyKey: "idem-1",
    provider: "anthropic" as const,
    submission: {
      model: Models.CLAUDE_HAIKU_4_5,
      prompt: ["hello"],
      agentsMd: [],
      files: [],
      mcpServers: []
    },
    secrets: { apiKeys: { anthropic: "sk-anthropic-test" } }
  };
}

function withSkills(skills: unknown) {
  const req = baseRequest();
  return { ...req, submission: { ...req.submission, skills } };
}

describe("parseSkills — name-only refs", () => {
  it("accepts a valid list and returns canonical {kind,name} refs", () => {
    expect(parseSkills([{ kind: "skill", name: "alpha" }, { kind: "skill", name: "beta-1" }])).toEqual([
      { kind: "skill", name: "alpha" },
      { kind: "skill", name: "beta-1" }
    ]);
  });

  it("treats undefined/null as an empty list", () => {
    expect(parseSkills(undefined)).toEqual([]);
    expect(parseSkills(null)).toEqual([]);
  });

  it("dedups by name", () => {
    expect(() => parseSkills([{ kind: "skill", name: "dup" }, { kind: "skill", name: "dup" }])).toThrow(
      /duplicate name: dup/
    );
  });

  it("rejects an unexpected field (e.g. a smuggled assetId)", () => {
    expect(() =>
      parseSkills([{ kind: "skill", name: "alpha", assetId: `asset_${"a".repeat(64)}` }])
    ).toThrow(/not an allowed field for a skill ref/);
  });

  it("rejects a bad pattern, the `__` separator, and reserved names", () => {
    expect(() => parseSkills([{ kind: "skill", name: "Bad Name" }])).toThrow(/must match/);
    expect(() => parseSkills([{ kind: "skill", name: "-lead" }])).toThrow(/must match/);
    expect(() => parseSkills([{ kind: "skill", name: "a__b" }])).toThrow(/"__"/);
    expect(() => parseSkills([{ kind: "skill", name: "skills" }])).toThrow(/reserved/);
    expect(() => parseSkills([{ kind: "skill", name: "skill" }])).toThrow(/reserved/);
  });

  it("bounds the list at SKILLS_MAX", () => {
    const many = Array.from({ length: SKILLS_MAX + 1 }, (_, i) => ({ kind: "skill", name: `s-${i}` }));
    expect(() => parseSkills(many)).toThrow(new RegExp(`${SKILLS_MAX}-skill limit`));
    const ok = Array.from({ length: SKILLS_MAX }, (_, i) => ({ kind: "skill", name: `s-${i}` }));
    expect(parseSkills(ok)).toHaveLength(SKILLS_MAX);
  });
});

describe("submission ingress — skills vs tools vs resolvedSkills", () => {
  it("parses submission.skills through the full request parser", () => {
    const parsed = parseSessionSubmissionRequest(withSkills([{ kind: "skill", name: "alpha" }]));
    expect(parsed.submission.skills).toEqual([{ kind: "skill", name: "alpha" }]);
  });

  it("rejects a kind:'skill' entry inside submission.tools with a redirect", () => {
    const req = baseRequest();
    expect(() =>
      parseSessionSubmissionRequest({
        ...req,
        submission: {
          ...req.submission,
          tools: [{ kind: "skill", assetId: `asset_${"a".repeat(64)}`, name: "alpha", description: "d" }]
        }
      })
    ).toThrow(/skills go in submission\.skills/);
  });

  it("rejects resolvedSkills on ingress but accepts it under trustedReparse", () => {
    const req = baseRequest();
    const withResolved = {
      ...req,
      submission: {
        ...req.submission,
        skills: [{ kind: "skill", name: "alpha" }],
        resolvedSkills: [
          { kind: "skill", assetId: `asset_${"a".repeat(64)}`, name: "alpha", description: "Alpha skill." }
        ]
      }
    };
    expect(() => parseSessionSubmissionRequest(withResolved)).toThrow(/platform-internal/);
    const reparsed = parseSessionSubmissionRequest(withResolved, { trustedReparse: true });
    expect(reparsed.submission.resolvedSkills).toEqual([
      { kind: "skill", assetId: `asset_${"a".repeat(64)}`, name: "alpha", description: "Alpha skill." }
    ]);
    // The public name-only refs survive alongside the trusted resolution.
    expect(reparsed.submission.skills).toEqual([{ kind: "skill", name: "alpha" }]);
  });
});

describe("idempotency — name-only skills into the hashed submission", () => {
  it("hashes EQUAL when only the (unhashed) bytes would differ — i.e. same name", () => {
    // The wire submission carries name-only refs, so 'different bytes, same name'
    // is not even expressible here: both requests serialise identically.
    const a = parseSessionSubmissionRequest(withSkills([{ kind: "skill", name: "alpha" }])).submission;
    const b = parseSessionSubmissionRequest(withSkills([{ kind: "skill", name: "alpha" }])).submission;
    expect(sha256(a)).toBe(sha256(b));
  });

  it("hashes DIFFERENTLY when the skill NAME differs", () => {
    const a = parseSessionSubmissionRequest(withSkills([{ kind: "skill", name: "alpha" }])).submission;
    const b = parseSessionSubmissionRequest(withSkills([{ kind: "skill", name: "beta" }])).submission;
    expect(sha256(a)).not.toBe(sha256(b));
  });

  it("resolvedSkills is derived, not hashed: it never appears on the ingress submission", () => {
    const parsed = parseSessionSubmissionRequest(withSkills([{ kind: "skill", name: "alpha" }])).submission;
    expect("resolvedSkills" in parsed).toBe(false);
  });
});

describe("skills meta-tool contract", () => {
  it("is named 'skills' and kept OUT of the public builtin tool set", () => {
    expect(SKILLS_TOOL_NAME).toBe("skills");
    expect(SKILLS_TOOL_DEFINITION.name).toBe("skills");
    expect((BUILTIN_TOOL_NAMES as readonly string[]).includes("skills")).toBe(false);
  });

  it("exposes an action enum of list/load with action required", () => {
    const schema = SKILLS_TOOL_DEFINITION.input_schema;
    expect(schema.properties.action.enum).toEqual(["list", "load"]);
    expect(schema.required).toEqual(["action"]);
    expect(schema.additionalProperties).toBe(false);
  });
});

describe("parseSkills — property: valid pattern in ⇒ echoed out, junk ⇒ throws", () => {
  it("round-trips pattern-valid, non-reserved names and never silently mangles", () => {
    const validName = fc
      .stringMatching(/^[a-z0-9][a-z0-9_-]{0,20}$/)
      .filter((n) => !n.includes("__") && n !== "skills" && n !== "skill");
    fc.assert(
      fc.property(fc.uniqueArray(validName, { maxLength: 8 }), (names) => {
        const refs = parseSkills(names.map((name) => ({ kind: "skill", name })));
        expect(refs).toEqual(names.map((name) => ({ kind: "skill", name })));
      })
    );
  });

  it("any name that violates the gate throws — never returns a bad ref", () => {
    const junk = fc.oneof(
      fc.constant("skills"),
      fc.constant("skill"),
      fc.constant("Has Space"),
      fc.constant("UPPER"),
      fc.constant("a__b"),
      fc.constant("-lead"),
      fc.string({ minLength: 1 }).filter((s) => !/^[a-z0-9][a-z0-9_-]{0,127}$/.test(s))
    );
    fc.assert(
      fc.property(junk, (name) => {
        expect(() => parseSkills([{ kind: "skill", name }])).toThrow();
      })
    );
  });
});
