import { describe, expect, it } from "vitest";
import {
  isAssetRef,
  isProviderSkillRef,
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
const goodAssetRef = {
  kind: "asset",
  assetId: `asset_${"a".repeat(64)}`,
  name: "rules"
} as const;
const goodMcpRef = { name: "github", url: "https://github.example.com/mcp" } as const;

const baseSubmission = {
  workspaceId: "workspace-1",
  idempotencyKey: "idem-1",
  submission: {
    model: "claude-haiku-4-5",
    prompt: "do the thing"
  },
  secrets: {
    apiKey: "sk-ant-x"
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

  it("parses an asset ref round-trip", () => {
    const parsed = parseSkillRef(goodAssetRef, "ref");
    expect(parsed).toEqual(goodAssetRef);
    expect(isAssetRef(parsed)).toBe(true);
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

  it("rejects a storage-specific ref", () => {
    const bad = {
      kind: "storage_backend",
      path: `assets/11111111-1111-4111-8111-111111111111/${"a".repeat(64)}`,
      hash: goodInlineHash,
      sizeBytes: 100,
      name: "rules"
    };
    expect(() => parseSkillRef(bad, "ref")).toThrow(/provider' or 'asset/);
  });

  it("rejects an asset ref with extra fields", () => {
    expect(() => parseSkillRef({ ...goodAssetRef, extra: 1 } as unknown, "ref")).toThrow(/unexpected/i);
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

  // SSRF deny-list parity with platform `denyReasonForHostIp`. The cases
  // below are the ones the hardened deny-list adds over the prior public
  // copy: IPv4-mapped IPv6 (both dotted and hex normalisation), CGNAT
  // (100.64.0.0/10), and ULA (fc00::/7). A mapped form must not smuggle a
  // private target past the literal checks.
  it.each([
    ["http://localhost/mcp", /loopback hostname/i],
    ["http://127.0.0.1/mcp", /loopback IPv4/i],
    ["http://169.254.169.254/mcp", /link-local IPv4/i],
    ["http://100.64.0.1/mcp", /CGNAT IPv4/i],
    ["http://[::ffff:127.0.0.1]/mcp", /loopback IPv4/i],
    ["http://[::ffff:169.254.169.254]/mcp", /link-local IPv4/i],
    ["http://[fc00::1]/mcp", /unique-local IPv6/i],
    ["http://[fd12:3456::1]/mcp", /unique-local IPv6/i]
  ])("rejects %s as an SSRF target", (url, pattern) => {
    expect(() => parseMcpServerRef({ name: "x", url }, "r")).toThrow(pattern);
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
      model: "claude-haiku-4-5",
      prompt: "do it",
      skills: [],
      mcpServers: []
    });
    expect(config.prompt).toBe("do it");
  });

  it("preserves a multi-part prompt array", () => {
    const config = parseRunRequestConfig({
      model: "claude-haiku-4-5",
      prompt: ["first turn", "follow up"],
      skills: [],
      mcpServers: []
    });
    expect(config.prompt).toEqual(["first turn", "follow up"]);
  });

  it("rejects an empty string prompt at the run-config boundary", () => {
    expect(() =>
      parseRunRequestConfig({ model: "claude-haiku-4-5", prompt: "", skills: [], mcpServers: [] })
    ).toThrow(/prompt/i);
  });

  it("rejects an empty string in a prompt array", () => {
    expect(() =>
      parseRunRequestConfig({
        model: "claude-haiku-4-5",
        prompt: ["first", ""],
        skills: [],
        mcpServers: []
      })
    ).toThrow(/prompt/i);
  });

  it("rejects extra top-level fields", () => {
    expect(() =>
      parseRunRequestConfig({
        model: "claude-haiku-4-5",
        prompt: "x",
        skills: [],
        mcpServers: [],
        extra: 1
      } as unknown)
    ).toThrow(/extra/i);
  });

  it("preserves environment, proxyEndpoints, and metadata pass-through fields", () => {
    const env = { networking: { mode: "limited" as const, allowedHosts: ["api.x.com"] } };
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
      model: "claude-haiku-4-5",
      prompt: "x",
      skills: [],
      mcpServers: [],
      environment: env,
      proxyEndpoints,
      metadata
    });
    expect(config.environment).toEqual(env);
    expect(config.proxyEndpoints).toEqual(proxyEndpoints);
    expect(config.metadata).toEqual(metadata);
  });

  it("accepts postHook config and preserves the public wire shape", () => {
    const config = parseRunRequestConfig({
      model: "claude-haiku-4-5",
      prompt: "x",
      postHook: {
        command: "pnpm test",
        timeout: "2m",
        maxTurns: 3,
        maxChars: 1000
      }
    });
    expect(config.postHook).toEqual({
      command: "pnpm test",
      timeout: "2m",
      maxTurns: 3,
      maxChars: 1000
    });
  });

  it("omits postHook config when command is empty", () => {
    const config = parseRunRequestConfig({
      model: "claude-haiku-4-5",
      prompt: "x",
      postHook: { command: "" }
    });
    expect(config.postHook).toBeUndefined();
  });

  it("rejects invalid postHook config at the run-config boundary", () => {
    expect(() =>
      parseRunRequestConfig({
        model: "claude-haiku-4-5",
        prompt: "x",
        postHook: { command: "pnpm test", maxTurns: -1 }
      })
    ).toThrow(/postHook\.maxTurns/);
    expect(() =>
      parseRunRequestConfig({
        model: "claude-haiku-4-5",
        prompt: "x",
        postHook: { command: "pnpm test", extra: true }
      })
    ).toThrow(/postHook\.extra/);
  });

  it("rejects removed cleanup config", () => {
    expect(() =>
      parseRunRequestConfig({
        model: "claude-haiku-4-5",
        prompt: "x",
        cleanup: { session: "delete" }
      })
    ).toThrow(/cleanup/);
  });

  it("rejects duplicate mcpServer names at the run-config boundary", () => {
    expect(() =>
      parseRunRequestConfig({
        model: "claude-haiku-4-5",
        prompt: "x",
        skills: [],
        mcpServers: [
          { name: "gh", url: "https://x" },
          { name: "gh", url: "https://y" }
        ]
      })
    ).toThrow(/duplicate/i);
  });

  it("accepts an asset skill and provider skill side by side", () => {
    const config = parseRunRequestConfig({
      model: "claude-haiku-4-5",
      prompt: "x",
      skills: [goodAssetRef, goodProviderRef],
      mcpServers: []
    });
    expect(config.skills).toHaveLength(2);
    const skills = config.skills ?? [];
    expect(isAssetRef(skills[0]!)).toBe(true);
    expect(isProviderSkillRef(skills[1]!)).toBe(true);
  });
});

describe("run-config — normaliseRunRequestConfig", () => {
  it("splits MCP headers out of the public field into the secret bundle", () => {
    const config: RunRequestConfig = {
      model: "claude-haiku-4-5",
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
      model: "claude-haiku-4-5",
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
      model: "claude-haiku-4-5",
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

  it("carries postHook through normalisation", () => {
    const config: RunRequestConfig = {
      model: "claude-haiku-4-5",
      prompt: "x",
      postHook: { command: "pnpm test", timeout: "1m", maxTurns: 1, maxChars: null }
    };
    expect(normaliseRunRequestConfig(config).postHook).toEqual(config.postHook);
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
    expect(parsed.secrets.apiKey).toBe("sk-ant-x");
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

  it("rejects provider-hosted skills in the submission", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseSubmission,
        submission: {
          ...baseSubmission.submission,
          skills: [goodAssetRef, goodProviderRef]
        }
      })
    ).toThrow(/managed runtime does not support/i);
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

  it("rejects duplicate asset skill ids", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseSubmission,
        submission: {
          ...baseSubmission.submission,
          skills: [goodAssetRef, goodAssetRef]
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

  it("accepts valid outputs.allowedDirs and normalises duplicates / trailing slashes", () => {
    const parsed = parseRunSubmissionRequest({
      ...baseSubmission,
      submission: {
        ...baseSubmission.submission,
        outputs: { allowedDirs: ["/workspace/outputs/", "/workspace/outputs", "/workspace/state"] }
      }
    });
    expect(parsed.submission.outputs?.allowedDirs).toEqual([
      "/workspace/outputs",
      "/workspace/state"
    ]);
  });

  it("omits outputs from the parsed submission when empty arrays are supplied", () => {
    const parsed = parseRunSubmissionRequest({
      ...baseSubmission,
      submission: {
        ...baseSubmission.submission,
        outputs: { allowedDirs: [], deniedDirs: [] }
      }
    });
    expect(parsed.submission.outputs).toBeUndefined();
  });

  it("accepts output capture limits and clamps capture timeout to the platform maximum", () => {
    const parsed = parseRunSubmissionRequest({
      ...baseSubmission,
      submission: {
        ...baseSubmission.submission,
        outputs: {
          captureTimeoutMs: 24 * 60 * 60 * 1000,
          maxFileBytes: 1_000_000_000_000,
          maxTotalBytes: 1_000_000_000_000,
          maxFiles: 50_000
        }
      }
    });
    expect(parsed.submission.outputs).toEqual({
      captureTimeoutMs: 6 * 60 * 60 * 1000,
      maxFileBytes: 1_000_000_000_000,
      maxTotalBytes: 1_000_000_000_000,
      maxFiles: 50_000
    });
  });

  it("rejects non-positive and non-integer output capture limits", () => {
    for (const [field, value] of [
      ["captureTimeoutMs", 0],
      ["maxFileBytes", -1],
      ["maxTotalBytes", 1.5],
      ["maxFiles", "50"]
    ] as const) {
      expect(() =>
        parseRunSubmissionRequest({
          ...baseSubmission,
          submission: {
            ...baseSubmission.submission,
            outputs: { [field]: value }
          }
        })
      ).toThrow(new RegExp(`submission\\.outputs\\.${field} must be a positive integer`));
    }
  });

  it("rejects outputs.allowedDirs entries that are not absolute paths", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseSubmission,
        submission: {
          ...baseSubmission.submission,
          outputs: { allowedDirs: ["relative/path"] }
        }
      })
    ).toThrow(/absolute UNIX path/);
  });

  it("rejects outputs.allowedDirs entries that contain '..'", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseSubmission,
        submission: {
          ...baseSubmission.submission,
          outputs: { allowedDirs: ["/workspace/../escape"] }
        }
      })
    ).toThrow(/'\.\.'/);
  });

  it("accepts outputs.deniedDirs — absolute subtree, bare segment, *.ext", () => {
    const parsed = parseRunSubmissionRequest({
      ...baseSubmission,
      submission: {
        ...baseSubmission.submission,
        outputs: { deniedDirs: ["/var/cache/", "node_modules", "*.tmp", "node_modules"] }
      }
    });
    expect(parsed.submission.outputs?.deniedDirs).toEqual(["/var/cache", "node_modules", "*.tmp"]);
  });

  it("rejects outputs.deniedDirs entries that contain '..'", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseSubmission,
        submission: { ...baseSubmission.submission, outputs: { deniedDirs: ["../escape"] } }
      })
    ).toThrow(/'\.\.'/);
  });

  it("rejects legacy top-level outputDirs/outputExcludes", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseSubmission,
        submission: { ...baseSubmission.submission, outputDirs: ["/workspace/out"] }
      })
    ).toThrow(/not an allowed field/);
    expect(() =>
      parseRunSubmissionRequest({
        ...baseSubmission,
        submission: { ...baseSubmission.submission, outputExcludes: ["node_modules"] }
      })
    ).toThrow(/not an allowed field/);
  });

  it("rejects outputs.allowedDirs entries with NUL bytes", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseSubmission,
        submission: {
          ...baseSubmission.submission,
          outputs: { allowedDirs: ["/workspace/\0evil"] }
        }
      })
    ).toThrow(/NUL/);
  });

  it("rejects outputs.allowedDirs lists with more than 32 entries", () => {
    const tooMany = Array.from({ length: 33 }, (_, i) => `/workspace/out-${i}`);
    expect(() =>
      parseRunSubmissionRequest({
        ...baseSubmission,
        submission: {
          ...baseSubmission.submission,
          outputs: { allowedDirs: tooMany }
        }
      })
    ).toThrow(/max is 32/);
  });

  it("rejects outputs.allowedDirs entries longer than 512 bytes", () => {
    const huge = "/" + "a".repeat(520);
    expect(() =>
      parseRunSubmissionRequest({
        ...baseSubmission,
        submission: {
          ...baseSubmission.submission,
          outputs: { allowedDirs: [huge] }
        }
      })
    ).toThrow(/exceeds 512 bytes/);
  });
});
