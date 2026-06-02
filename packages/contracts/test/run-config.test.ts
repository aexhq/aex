import { describe, expect, it } from "vitest";
import {
  isProviderSkillRef,
  isR2SkillRef,
  MCP_SERVER_NAME_PATTERN,
  normaliseSkillBundlePath,
  normaliseRunRequestConfig,
  parseRunRequestConfig,
  parseRunSubmissionRequest,
  parseMcpServerRef,
  parseSkillRef,
  SKILL_BUNDLE_LIMITS,
  SKILL_ID_PATTERN,
  SKILL_NAME_PATTERN,
  SkillBundleValidationError,
  INLINE_CONTENT_HASH_PATTERN,
  validateSkillBundleEntry,
  validateSkillBundleManifest,
  type RunRequestConfig,
  type SkillRef
} from "../src/index.js";

const goodSkillId = "skl_abcdefgh01234567";
const goodProviderRef = {
  kind: "provider",
  vendor: "anthropic",
  skillId: "pdf",
  version: "v1"
} as const;
const goodInlineHash = `sha256:${"a".repeat(64)}`;
const goodWorkspaceUuid = "11111111-1111-4111-8111-111111111111";
const goodR2Ref = {
  kind: "r2",
  path: `assets/${goodWorkspaceUuid}/${"a".repeat(64)}`,
  hash: goodInlineHash,
  sizeBytes: 100,
  name: "rules"
} as const;
const goodMcpRef = { name: "github", url: "https://github.example.com/mcp" } as const;

const baseSubmission = {
  workspaceId: "workspace-1",
  idempotencyKey: "idem-1",
  submission: {
    model: "claude-sonnet-4-5-20250929",
    prompt: "do the thing"
  },
  secrets: {
    anthropic: { apiKey: "sk-ant-x" }
  }
} as const;

describe("run-config — id and name patterns", () => {
  it("accepts the canonical skl_ id format", () => {
    expect(SKILL_ID_PATTERN.test(goodSkillId)).toBe(true);
  });

  it("rejects ids missing the skl_ prefix", () => {
    expect(SKILL_ID_PATTERN.test("abc_abcdefgh")).toBe(false);
  });

  it("rejects names with uppercase characters", () => {
    expect(SKILL_NAME_PATTERN.test("RulesPack")).toBe(false);
  });

  it("accepts kebab and snake names", () => {
    expect(SKILL_NAME_PATTERN.test("company-rules")).toBe(true);
    expect(SKILL_NAME_PATTERN.test("company_rules")).toBe(true);
  });

  it("rejects MCP names starting with a digit", () => {
    expect(MCP_SERVER_NAME_PATTERN.test("9pin")).toBe(false);
  });
});

describe("run-config — parseSkillRef", () => {
  it("parses a provider ref round-trip", () => {
    const parsed = parseSkillRef(goodProviderRef, "ref");
    expect(parsed).toEqual(goodProviderRef);
    expect(isProviderSkillRef(parsed)).toBe(true);
  });

  it("parses an r2 ref round-trip", () => {
    const parsed = parseSkillRef(goodR2Ref, "ref");
    expect(parsed).toEqual(goodR2Ref);
    expect(isR2SkillRef(parsed)).toBe(true);
  });

  it("rejects an unknown kind", () => {
    expect(() => parseSkillRef({ kind: "workspace", id: goodSkillId }, "ref")).toThrow(/kind/i);
    expect(() => parseSkillRef({ kind: "inline", slot: "x", name: "n", contentHash: goodInlineHash }, "ref")).toThrow(/kind/i);
  });

  it("rejects a provider ref with unknown vendor", () => {
    expect(() =>
      parseSkillRef({ kind: "provider", vendor: "openai", skillId: "x" }, "ref")
    ).toThrow(/vendor/i);
  });

  it("rejects an r2 ref whose path/hash disagree", () => {
    const bad = { ...goodR2Ref, hash: `sha256:${"b".repeat(64)}` };
    expect(() => parseSkillRef(bad, "ref")).toThrow(/hash segment/);
  });

  it("rejects an r2 ref with extra fields", () => {
    expect(() => parseSkillRef({ ...goodR2Ref, extra: 1 } as unknown, "ref")).toThrow(/unexpected/i);
  });

  it("INLINE_CONTENT_HASH_PATTERN is exported + matches sha256:<64-hex>", () => {
    expect(INLINE_CONTENT_HASH_PATTERN.test(goodInlineHash)).toBe(true);
    expect(INLINE_CONTENT_HASH_PATTERN.test("sha256:notlongenough")).toBe(false);
  });
});

describe("run-config — parseMcpServerRef", () => {
  it("accepts http and https urls", () => {
    expect(parseMcpServerRef({ name: "x", url: "http://example.com" }, "r").url).toBe(
      "http://example.com"
    );
    expect(parseMcpServerRef({ name: "x", url: "https://example.com" }, "r").url).toBe(
      "https://example.com"
    );
  });

  it("rejects non-http schemes", () => {
    expect(() => parseMcpServerRef({ name: "x", url: "file:///etc" }, "r")).toThrow(/url/i);
  });

  it("rejects names that violate the pattern", () => {
    expect(() =>
      parseMcpServerRef({ name: "Bad Name", url: "https://x" }, "r")
    ).toThrow(/name/i);
  });

  it("strips headers from the non-secret ref (headers live on RunConfigMcpServer)", () => {
    // CRITICAL: McpServerRef is the non-secret wire half. Accepting and
    // dropping `headers` would let a caller inline credentials into the
    // submission.mcpServers payload that the secret-bearing-field scan
    // never sees. Reject loudly so the call site has to use
    // `RunConfigMcpServer` + `normaliseRunRequestConfig` instead.
    expect(() =>
      parseMcpServerRef(
        { name: "x", url: "https://x", headers: { Authorization: "Bearer y" } },
        "r"
      )
    ).toThrow(/headers/i);
  });

  it("rejects any unknown field", () => {
    expect(() =>
      parseMcpServerRef({ name: "x", url: "https://x", extra: 1 }, "r")
    ).toThrow(/extra/i);
  });

  it("rejects URLs containing userinfo (credentials in URL)", () => {
    // Auth must live in `secrets.mcpServers[].headers`, never in the URL
    // itself — userinfo would otherwise be persisted in the non-secret
    // run snapshot and hashed into the idempotency key.
    expect(() =>
      parseMcpServerRef({ name: "x", url: "https://user:pass@example.com" }, "r")
    ).toThrow(/userinfo|username|password/i);
    expect(() =>
      parseMcpServerRef({ name: "x", url: "https://user@example.com" }, "r")
    ).toThrow(/userinfo|username|password/i);
  });
});

describe("run-config — normaliseSkillBundlePath", () => {
  it("rejects path traversal", () => {
    expect(() => normaliseSkillBundlePath("foo/../bar")).toThrow(SkillBundleValidationError);
  });

  it("rejects absolute paths", () => {
    expect(() => normaliseSkillBundlePath("/etc/passwd")).toThrow(SkillBundleValidationError);
  });

  it("rejects windows drive letters", () => {
    expect(() => normaliseSkillBundlePath("C:/Users/x")).toThrow(SkillBundleValidationError);
  });

  it("rejects backslash paths", () => {
    expect(() => normaliseSkillBundlePath("foo\\bar")).toThrow(SkillBundleValidationError);
  });

  it("rejects NUL bytes", () => {
    expect(() => normaliseSkillBundlePath("foo\u0000bar")).toThrow(SkillBundleValidationError);
  });

  it("rejects depth greater than the cap", () => {
    const tooDeep = Array.from({ length: SKILL_BUNDLE_LIMITS.maxDepth + 1 }, (_, i) => `d${i}`).join("/") + "/SKILL.md";
    expect(() => normaliseSkillBundlePath(tooDeep)).toThrow(SkillBundleValidationError);
  });

  it("rejects paths longer than the cap", () => {
    const tooLong = "a".repeat(SKILL_BUNDLE_LIMITS.maxPathLength + 1);
    expect(() => normaliseSkillBundlePath(tooLong)).toThrow(SkillBundleValidationError);
  });

  it("normalises a clean path unchanged", () => {
    expect(normaliseSkillBundlePath("scripts/check.py")).toBe("scripts/check.py");
  });
});

describe("run-config — validateSkillBundleEntry", () => {
  it("sanitises mode to 0o644 for files", () => {
    const entry = validateSkillBundleEntry({ path: "scripts/x.py", size: 1, mode: 0o777 });
    expect(entry.mode).toBe(0o644);
  });

  it("sanitises mode to 0o755 when callers supply 0o755", () => {
    const entry = validateSkillBundleEntry({ path: "scripts/x", size: 0, mode: 0o755 });
    expect(entry.mode).toBe(0o755);
  });

  it("rejects negative size", () => {
    expect(() => validateSkillBundleEntry({ path: "a", size: -1, mode: 0o644 })).toThrow(SkillBundleValidationError);
  });
});

describe("run-config — validateSkillBundleManifest", () => {
  // SKILL.md is restored as a strict precondition for skill bundles.
  // Per the May-2026 decision log in agent-context-uploads.md, a
  // bundle without SKILL.md is not a skill — it goes through the
  // separate `AgentsMd` or `File` concepts. Skills mean Claude
  // Skills.
  it("requires SKILL.md at the root", () => {
    expect(() =>
      validateSkillBundleManifest([{ path: "scripts/x.py", size: 1, mode: 0o644 }])
    ).toThrow(/SKILL\.md/);
  });

  it("rejects duplicate paths", () => {
    expect(() =>
      validateSkillBundleManifest([
        { path: "SKILL.md", size: 1, mode: 0o644 },
        { path: "scripts/x.py", size: 1, mode: 0o644 },
        { path: "scripts/x.py", size: 2, mode: 0o644 }
      ])
    ).toThrow(/duplicate/i);
  });

  it("rejects manifests over the file count cap", () => {
    const entries = [{ path: "SKILL.md", size: 1, mode: 0o644 }] as Array<{
      path: string;
      size: number;
      mode: number;
    }>;
    for (let i = 0; i < SKILL_BUNDLE_LIMITS.maxFiles; i += 1) {
      entries.push({ path: `scripts/f${i}.py`, size: 1, mode: 0o644 });
    }
    expect(() => validateSkillBundleManifest(entries)).toThrow(/maxFiles|file count/i);
  });

  it("rejects bundles over the size cap", () => {
    const half = Math.ceil(SKILL_BUNDLE_LIMITS.maxDecompressedBytes / 2);
    expect(() =>
      validateSkillBundleManifest([
        { path: "SKILL.md", size: half, mode: 0o644 },
        { path: "scripts/big.bin", size: half + 1, mode: 0o644 }
      ])
    ).toThrow(/size/i);
  });

  it("returns a normalised manifest with totals and counts", () => {
    const manifest = validateSkillBundleManifest([
      { path: "scripts/x.py", size: 5, mode: 0o644 },
      { path: "SKILL.md", size: 10, mode: 0o644 }
    ]);
    // Entries are returned in input order; SKILL.md just has to exist
    // somewhere — the bundle packager will dictate the order on disk.
    const paths = manifest.entries.map((e) => e.path);
    expect(paths).toContain("SKILL.md");
    expect(paths).toContain("scripts/x.py");
    expect(manifest.fileCount).toBe(2);
    expect(manifest.totalSize).toBe(15);
  });
});

describe("run-config — parseRunRequestConfig", () => {
  it("preserves a string prompt verbatim (normalisation happens at submission time)", () => {
    const config = parseRunRequestConfig({
      model: "claude-sonnet-4-5",
      prompt: "do it",
      skills: [],
      mcpServers: []
    });
    expect(config.prompt).toBe("do it");
  });

  it("preserves a multi-part prompt array", () => {
    const config = parseRunRequestConfig({
      model: "claude-sonnet-4-5",
      prompt: ["first turn", "follow up"],
      skills: [],
      mcpServers: []
    });
    expect(config.prompt).toEqual(["first turn", "follow up"]);
  });

  it("rejects an empty string prompt at the run-config boundary", () => {
    expect(() =>
      parseRunRequestConfig({ model: "claude-sonnet-4-5", prompt: "", skills: [], mcpServers: [] })
    ).toThrow(/prompt/i);
  });

  it("rejects an empty string in a prompt array", () => {
    expect(() =>
      parseRunRequestConfig({
        model: "claude-sonnet-4-5",
        prompt: ["first", ""],
        skills: [],
        mcpServers: []
      })
    ).toThrow(/prompt/i);
  });

  it("rejects extra top-level fields", () => {
    expect(() =>
      parseRunRequestConfig({
        model: "claude-sonnet-4-5",
        prompt: "x",
        skills: [],
        mcpServers: [],
        extra: 1
      } as unknown)
    ).toThrow(/extra/i);
  });

  it("preserves environment, cleanup, proxyEndpoints, and metadata pass-through fields", () => {
    const env = { networking: { mode: "limited" as const, allowedHosts: ["api.x.com"] } };
    const cleanup = { session: "delete" as const };
    const proxyEndpoints = [
      {
        name: "stripe",
        baseUrl: "https://api.stripe.com",
        authShape: { type: "bearer" as const },
        allowMethods: ["GET"],
        allowPathPrefixes: ["/v1/"]
      }
    ];
    const metadata = { ticket: "ANT-1" };
    const config = parseRunRequestConfig({
      model: "claude-sonnet-4-5",
      prompt: "x",
      skills: [],
      mcpServers: [],
      environment: env,
      cleanup,
      proxyEndpoints,
      metadata
    });
    expect(config.environment).toEqual(env);
    expect(config.cleanup).toEqual(cleanup);
    expect(config.proxyEndpoints).toEqual(proxyEndpoints);
    expect(config.metadata).toEqual(metadata);
  });

  it("rejects duplicate mcpServer names at the run-config boundary", () => {
    expect(() =>
      parseRunRequestConfig({
        model: "claude-sonnet-4-5",
        prompt: "x",
        skills: [],
        mcpServers: [
          { name: "gh", url: "https://x" },
          { name: "gh", url: "https://y" }
        ]
      })
    ).toThrow(/duplicate/i);
  });

  it("accepts an r2 skill and provider skill side by side", () => {
    const config = parseRunRequestConfig({
      model: "claude-sonnet-4-5",
      prompt: "x",
      skills: [goodR2Ref, goodProviderRef],
      mcpServers: []
    });
    expect(config.skills).toHaveLength(2);
    const skills = config.skills ?? [];
    expect(isR2SkillRef(skills[0]!)).toBe(true);
    expect(isProviderSkillRef(skills[1]!)).toBe(true);
  });
});

describe("run-config — normaliseRunRequestConfig", () => {
  it("splits MCP headers out of the public field into the secret bundle", () => {
    const config: RunRequestConfig = {
      model: "claude-sonnet-4-5",
      prompt: ["x"],
      skills: [],
      mcpServers: [
        { name: "gh", url: "https://x", headers: { Authorization: "Bearer y" } }
      ]
    };
    const norm = normaliseRunRequestConfig(config);
    expect(norm.mcpServers).toEqual([{ name: "gh", url: "https://x" }]);
    expect(norm.mcpServerSecrets).toEqual([
      { name: "gh", url: "https://x", headers: { Authorization: "Bearer y" } }
    ]);
  });

  it("returns an empty mcpServerSecrets array when no headers were provided", () => {
    const config: RunRequestConfig = {
      model: "claude-sonnet-4-5",
      prompt: "x",
      skills: [],
      mcpServers: [{ name: "noauth", url: "https://x" }]
    };
    const norm = normaliseRunRequestConfig(config);
    expect(norm.mcpServers).toEqual([{ name: "noauth", url: "https://x" }]);
    expect(norm.mcpServerSecrets).toEqual([]);
  });

  it("includes only entries whose run-config entry had headers in mcpServerSecrets", () => {
    const config: RunRequestConfig = {
      model: "claude-sonnet-4-5",
      prompt: "x",
      skills: [],
      mcpServers: [
        { name: "noauth", url: "https://x" },
        { name: "gh", url: "https://y", headers: { Authorization: "Bearer z" } }
      ]
    };
    const norm = normaliseRunRequestConfig(config);
    expect(norm.mcpServers).toEqual([
      { name: "noauth", url: "https://x" },
      { name: "gh", url: "https://y" }
    ]);
    expect(norm.mcpServerSecrets.map((s) => s.name)).toEqual(["gh"]);
  });
});

describe("run-config — parseRunSubmissionRequest", () => {
  it("accepts the canonical happy path", () => {
    const parsed = parseRunSubmissionRequest(baseSubmission);
    expect(parsed.workspaceId).toBe("workspace-1");
    expect(parsed.idempotencyKey).toBe("idem-1");
    expect(parsed.submission.prompt).toEqual(["do the thing"]);
    expect(parsed.submission.skills).toEqual([]);
    expect(parsed.submission.mcpServers).toEqual([]);
    expect(parsed.secrets.anthropic?.apiKey).toBe("sk-ant-x");
  });

  it("normalises a string prompt into an array on the wire", () => {
    const parsed = parseRunSubmissionRequest({
      ...baseSubmission,
      submission: {
        ...baseSubmission.submission,
        prompt: "first"
      }
    });
    expect(parsed.submission.prompt).toEqual(["first"]);
  });

  it("rejects secret-bearing fields at the top level", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseSubmission,
        apiKey: "leak"
      })
    ).toThrow(/secret/i);
  });

  it("rejects unknown fields in submission", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseSubmission,
        submission: {
          ...baseSubmission.submission,
          extra: 1
        }
      })
    ).toThrow(/extra/i);
  });

  it("rejects an mcpServers secret with no matching submission entry", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseSubmission,
        submission: {
          ...baseSubmission.submission,
          mcpServers: [{ name: "gh", url: "https://x" }]
        },
        secrets: {
          ...baseSubmission.secrets,
          mcpServers: [{ name: "orphan", url: "https://x", headers: { a: "b" } }]
        }
      })
    ).toThrow(/orphan/i);
  });

  it("rejects secrets.mcpServers when URL diverges from submission.mcpServers", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseSubmission,
        submission: {
          ...baseSubmission.submission,
          mcpServers: [{ name: "gh", url: "https://a.example.com" }]
        },
        secrets: {
          ...baseSubmission.secrets,
          mcpServers: [{ name: "gh", url: "https://b.example.com", headers: { a: "b" } }]
        }
      })
    ).toThrow(/url/i);
  });

  it("rejects duplicate names within secrets.mcpServers", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseSubmission,
        submission: {
          ...baseSubmission.submission,
          mcpServers: [{ name: "gh", url: "https://x" }]
        },
        secrets: {
          ...baseSubmission.secrets,
          mcpServers: [
            { name: "gh", url: "https://x", headers: { Authorization: "Bearer a" } },
            { name: "gh", url: "https://x", headers: { Authorization: "Bearer b" } }
          ]
        }
      })
    ).toThrow(/duplicate/i);
  });

  it("rejects duplicate mcpServers entries in the submission", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseSubmission,
        submission: {
          ...baseSubmission.submission,
          mcpServers: [
            { name: "gh", url: "https://x" },
            { name: "gh", url: "https://y" }
          ]
        }
      })
    ).toThrow(/duplicate/i);
  });

  it("accepts a workspace skill plus a provider skill in the submission", () => {
    const parsed = parseRunSubmissionRequest({
      ...baseSubmission,
      submission: {
        ...baseSubmission.submission,
        skills: [goodR2Ref, goodProviderRef]
      }
    });
    const skills = parsed.submission.skills as readonly SkillRef[];
    expect(skills).toHaveLength(2);
    expect(skills[0]).toEqual(goodR2Ref);
    expect(skills[1]).toEqual(goodProviderRef);
  });

  it("rejects an empty string prompt", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseSubmission,
        submission: {
          ...baseSubmission.submission,
          prompt: ""
        }
      })
    ).toThrow(/prompt/i);
  });

  it("rejects duplicate r2 skill paths", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseSubmission,
        submission: {
          ...baseSubmission.submission,
          skills: [goodR2Ref, goodR2Ref]
        }
      })
    ).toThrow(/duplicate/i);
  });

  it("rejects duplicate provider skills (vendor:skillId:version triple)", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseSubmission,
        submission: {
          ...baseSubmission.submission,
          skills: [goodProviderRef, goodProviderRef]
        }
      })
    ).toThrow(/duplicate/i);
  });

  it("accepts a valid outputDirs list and normalises duplicates / trailing slashes", () => {
    const parsed = parseRunSubmissionRequest({
      ...baseSubmission,
      submission: {
        ...baseSubmission.submission,
        outputDirs: ["/workspace/outputs/", "/workspace/outputs", "/workspace/state"]
      }
    });
    expect(parsed.submission.outputDirs).toEqual([
      "/workspace/outputs",
      "/workspace/state"
    ]);
  });

  it("omits outputDirs from the parsed submission when an empty array is supplied", () => {
    const parsed = parseRunSubmissionRequest({
      ...baseSubmission,
      submission: {
        ...baseSubmission.submission,
        outputDirs: []
      }
    });
    expect(parsed.submission.outputDirs).toBeUndefined();
  });

  it("rejects outputDirs entries that are not absolute paths", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseSubmission,
        submission: {
          ...baseSubmission.submission,
          outputDirs: ["relative/path"]
        }
      })
    ).toThrow(/absolute UNIX path/);
  });

  it("rejects outputDirs entries that contain '..'", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseSubmission,
        submission: {
          ...baseSubmission.submission,
          outputDirs: ["/workspace/../escape"]
        }
      })
    ).toThrow(/'\.\.'/);
  });

  it("rejects outputDirs entries with NUL bytes", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseSubmission,
        submission: {
          ...baseSubmission.submission,
          outputDirs: ["/workspace/\0evil"]
        }
      })
    ).toThrow(/NUL/);
  });

  it("rejects outputDirs lists with more than 32 entries", () => {
    const tooMany = Array.from({ length: 33 }, (_, i) => `/workspace/out-${i}`);
    expect(() =>
      parseRunSubmissionRequest({
        ...baseSubmission,
        submission: {
          ...baseSubmission.submission,
          outputDirs: tooMany
        }
      })
    ).toThrow(/max is 32/);
  });

  it("rejects outputDirs entries longer than 512 bytes", () => {
    const huge = "/" + "a".repeat(520);
    expect(() =>
      parseRunSubmissionRequest({
        ...baseSubmission,
        submission: {
          ...baseSubmission.submission,
          outputDirs: [huge]
        }
      })
    ).toThrow(/exceeds 512 bytes/);
  });
});
