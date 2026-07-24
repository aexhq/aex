import { describe, expect, it } from "bun:test";
import * as publicContracts from "../src/index.js";
import {
  packageInstallString,
  SESSION_TERMINAL_OUTCOMES
} from "../src/index.js";
import {
  getSessionWorkflowStatusKind,
  isTerminalSessionWorkflowStatus,
  parseSessionSubmissionRequest,
  TERMINAL_SESSION_WORKFLOW_STATUSES
} from "../src/internal.js";
// @ts-expect-error The platform parser input is available only from the internal entrypoint.
import type { PlatformSessionSubmissionInput } from "../src/index.js";
void (undefined as unknown as PlatformSessionSubmissionInput);

describe("platform status contracts", () => {
  it("keeps orchestration workflow states out of the public entrypoint", () => {
    expect(publicContracts).not.toHaveProperty("SESSION_WORKFLOW_STATUSES");
    expect(publicContracts).not.toHaveProperty("TERMINAL_SESSION_WORKFLOW_STATUSES");
    expect(publicContracts).not.toHaveProperty("getSessionWorkflowStatusKind");
  });

  it("classifies terminal and active session statuses", () => {
    expect(isTerminalSessionWorkflowStatus("succeeded")).toBe(true);
    expect(isTerminalSessionWorkflowStatus("cleanup_failed")).toBe(true);
    expect(isTerminalSessionWorkflowStatus("provider_running")).toBe(false);
    expect(getSessionWorkflowStatusKind("queued")).toBe("active");
    expect(getSessionWorkflowStatusKind("cleanup_failed")).toBe("terminal");
  });

  it("SESSION_TERMINAL_OUTCOMES is exactly the five run outcomes", () => {
    expect(new Set(SESSION_TERMINAL_OUTCOMES)).toEqual(
      new Set(["succeeded", "failed", "timed_out", "cancelled", "interrupted"])
    );
  });

  it("run outcomes are a strict subset of the read-terminal set", () => {
    const readTerminal = new Set<string>(TERMINAL_SESSION_WORKFLOW_STATUSES);
    for (const o of SESSION_TERMINAL_OUTCOMES) {
      expect(readTerminal.has(o)).toBe(true);
    }
    // The read-terminal set additionally carries the post-terminal
    // housekeeping states the funnel never writes as an outcome.
    expect(TERMINAL_SESSION_WORKFLOW_STATUSES.length).toBeGreaterThan(SESSION_TERMINAL_OUTCOMES.length);
    expect(readTerminal.has("cleanup_failed")).toBe(true);
    expect(new Set<string>(SESSION_TERMINAL_OUTCOMES).has("cleanup_failed")).toBe(false);
  });
});

describe("platform session submission schema", () => {
  const baseSecrets = {} as const;
  const baseSubmission = {
    model: "anthropic/claude-haiku-4-5",
    prompt: ["say hello"],
    assets: { files: [], skills: [], tools: [], instructions: [] },
    builtinTools: "default",
    mcpServers: []
  } as const;

  it("parses the minimal session submission contract", () => {
    const parsed = parseSessionSubmissionRequest({
      workspaceId: "workspace-1",
      idempotencyKey: "idem-1",
      submission: { ...baseSubmission, metadata: { topic: "platform" } },
      secrets: baseSecrets
    });

    expect(parsed.submission.model).toBe("anthropic/claude-haiku-4-5");
    expect(parsed.submission.prompt).toEqual(["say hello"]);
    expect(parsed.submission.metadata?.topic).toBe("platform");
    expect(parsed.secrets).toEqual({});
  });

  it("accepts any gateway model slug with no provider and no keys", () => {
    const parsed = parseSessionSubmissionRequest({
      workspaceId: "workspace-1",
      idempotencyKey: "idem-1",
      submission: { ...baseSubmission, model: "deepseek/deepseek-v4-flash" },
      secrets: {}
    });

    expect(parsed.submission.model).toBe("deepseek/deepseek-v4-flash");
  });

  it("rejects the removed cleanup policy field", () => {
    const base = {
      workspaceId: "workspace-1",
      idempotencyKey: "idem-1",
      submission: baseSubmission,
      secrets: baseSecrets
    };

    expect(parseSessionSubmissionRequest(base).submission.prompt).toEqual(["say hello"]);
    expect(() => parseSessionSubmissionRequest({ ...base, cleanup: { session: "delete" } })).toThrow(/cleanup/);
  });

  it("parses the platform.systemPrompt opt-out and rejects bad shapes", () => {
    const base = {
      workspaceId: "workspace-1",
      idempotencyKey: "idem-1",
      secrets: baseSecrets
    };

    // Omitted → field absent (default injection stays on downstream).
    expect(parseSessionSubmissionRequest({ ...base, submission: baseSubmission }).submission.platform).toBeUndefined();

    // Explicit "off" survives parsing.
    expect(
      parseSessionSubmissionRequest({
        ...base,
        submission: { ...baseSubmission, platform: { systemPrompt: "off" } }
      }).submission.platform
    ).toEqual({ systemPrompt: "off" });

    // Invalid enum value rejected.
    expect(() =>
      parseSessionSubmissionRequest({
        ...base,
        submission: { ...baseSubmission, platform: { systemPrompt: "yes" } }
      })
    ).toThrow(/platform\.systemPrompt/);

    // Unknown nested key rejected.
    expect(() =>
      parseSessionSubmissionRequest({
        ...base,
        submission: { ...baseSubmission, platform: { outputDir: "/x" } }
      })
    ).toThrow(/platform\.outputDir/);
  });

  it("rejects invalid submission shapes", () => {
    expect(() => parseSessionSubmissionRequest({
      workspaceId: "workspace-1",
      submission: baseSubmission,
      secrets: baseSecrets
    })).toThrow(/idempotencyKey/);
  });

  it("accepts an omitted secrets block (managed keys — a run needs no key)", () => {
    const parsed = parseSessionSubmissionRequest({
      workspaceId: "workspace-1",
      idempotencyKey: "idem-1",
      submission: baseSubmission
    });
    expect(parsed.secrets).toEqual({});
  });

  it("rejects secrets.apiKeys as a removed field (managed keys only)", () => {
    expect(() => parseSessionSubmissionRequest({
      workspaceId: "workspace-1",
      idempotencyKey: "idem-1",
      submission: baseSubmission,
      secrets: { apiKeys: { anthropic: "sk-ant-test" } }
    })).toThrow(/secrets\.apiKeys is not an allowed field; permitted: mcpServers, envSecrets/);
  });

  it("accepts mcpServers inside the secrets block", () => {
    const parsed = parseSessionSubmissionRequest({
      workspaceId: "workspace-1",
      idempotencyKey: "idem-1",
      submission: {
        ...baseSubmission,
        mcpServers: [{ name: "files", url: "https://mcp.example.test" }]
      },
      secrets: {
        mcpServers: [{ name: "files", url: "https://mcp.example.test", headers: { authorization: "Bearer x" } }]
      }
    });

    expect(parsed.secrets.mcpServers?.[0]?.url).toBe("https://mcp.example.test");
  });

  it("accepts custom tools as version-pinned workspace resources", () => {
    const tool = {
      kind: "tool" as const,
      resourceId: `wres_${"1".repeat(32)}`,
      version: 1,
      assetId: `asset_${"a".repeat(64)}`,
      contentHash: `sha256:${"a".repeat(64)}`,
      name: "calendar_lookup",
      description: "Looks up calendar availability.",
      input_schema: {
        type: "object",
        properties: { start: { type: "string" } },
        required: ["start"]
      },
      entry: "index.js"
    };
    const parsed = parseSessionSubmissionRequest({
      workspaceId: "ws_123",
      idempotencyKey: "idem-tool",
      submission: {
        model: "anthropic/claude-haiku-4-5",
        prompt: "use the lookup tool",
        assets: { files: [], skills: [], tools: [tool], instructions: [] },
        builtinTools: "default",
        mcpServers: []
      },
      secrets: baseSecrets
    });

    expect(parsed.submission.assets.tools).toEqual([tool]);
  });

  it("rejects user tool names that collide with MCP namespace routing", () => {
    expect(() =>
      parseSessionSubmissionRequest({
        workspaceId: "ws_123",
        idempotencyKey: "idem-tool",
        submission: {
          model: "anthropic/claude-haiku-4-5",
          prompt: "use the lookup tool",
          assets: { files: [], skills: [], instructions: [], tools: [
            {
              kind: "tool",
              resourceId: `wres_${"1".repeat(32)}`,
              version: 1,
              assetId: `asset_${"a".repeat(64)}`,
              contentHash: `sha256:${"a".repeat(64)}`,
              name: "calendar__lookup",
              description: "Looks up calendar availability.",
              input_schema: { type: "object", properties: {}, required: [] },
              entry: "index.js"
            }
          ] },
          builtinTools: "default",
          mcpServers: []
        },
        secrets: baseSecrets
      })
    ).toThrow(/non-reserved tool name/);
  });

  it("rejects stdio-shaped MCP servers with the canonical remote-only error", () => {
    const expected =
      "stdio MCP servers are not supported by Aex. Aex supports remote MCP servers over HTTP/SSE only.";

    expect(() => parseSessionSubmissionRequest({
      workspaceId: "workspace-1",
      idempotencyKey: "idem-1",
      submission: {
        ...baseSubmission,
        mcpServers: [{ name: "local", transport: "stdio", command: "npx" }]
      },
      secrets: baseSecrets
    })).toThrow(expected);

    expect(() => parseSessionSubmissionRequest({
      workspaceId: "workspace-1",
      idempotencyKey: "idem-1",
      submission: {
        ...baseSubmission,
        mcpServers: [{ name: "local", url: "https://mcp.example.test", command: "node" }]
      },
      secrets: baseSecrets
    })).toThrow(expected);
  });

  it("rejects unknown keys inside secrets", () => {
    expect(() => parseSessionSubmissionRequest({
      workspaceId: "workspace-1",
      idempotencyKey: "idem-1",
      submission: baseSubmission,
      secrets: { openai: { apiKey: "x" } }
    })).toThrow(
      /secrets\.openai is not an allowed field; permitted: mcpServers, envSecrets/
    );
  });

  it("rejects secret-bearing fields outside the secrets allowlist", () => {
    expect(() => parseSessionSubmissionRequest({
      workspaceId: "workspace-1",
      idempotencyKey: "idem-1",
      providerApiKey: "sk-ant-test",
      submission: baseSubmission,
      secrets: baseSecrets
    })).toThrow(/providerApiKey/);
  });

  it("rejects a whitespace-only prompt that would trim to empty downstream", () => {
    const submit = (prompt: unknown) =>
      parseSessionSubmissionRequest({
        workspaceId: "workspace-1",
        idempotencyKey: "idem-1",
        submission: { ...baseSubmission, prompt },
        secrets: baseSecrets
      });

    expect(() => submit([" "])).toThrow(/non-whitespace/);
    expect(() => submit(["\n\t "])).toThrow(/non-whitespace/);
    expect(() => submit(" ")).toThrow(/non-whitespace/);
    // An ordinary prompt still parses (and is stored untrimmed).
    expect(submit(["say hello"]).submission.prompt).toEqual(["say hello"]);
  });
});

describe("environment.packages ecosystem parsing", () => {
  const base = {
    workspaceId: "workspace-1",
    idempotencyKey: "idem-1",
    secrets: {}
  } as const;
  const baseSubmission = {
    model: "anthropic/claude-haiku-4-5",
    prompt: ["say hello"],    assets: { files: [], skills: [], tools: [], instructions: [] },
      builtinTools: "default",
    mcpServers: []
  } as const;

  const parsePackages = (packages: unknown) =>
    parseSessionSubmissionRequest({
      ...base,
      submission: { ...baseSubmission, environment: { packages } }
    }).submission.environment?.packages;

  it("parses an ecosystem prefix and strips it to the bare package name", () => {
    expect(parsePackages([{ name: "pip:pandas", version: "2.2.0" }])).toEqual([
      { name: "pandas", version: "2.2.0", ecosystem: "pip" }
    ]);
    expect(parsePackages([{ name: "npm:express" }])).toEqual([
      { name: "express", ecosystem: "npm" }
    ]);
  });

  it("defaults an unprefixed package name to the apt ecosystem", () => {
    expect(parsePackages([{ name: "ffmpeg" }])).toEqual([
      { name: "ffmpeg", ecosystem: "apt" }
    ]);
  });

  it("rejects an unknown ecosystem prefix", () => {
    expect(() => parsePackages([{ name: "conda:numpy" }])).toThrow(/unknown ecosystem prefix "conda:"/);
  });

  it("rejects a package whose name is empty after stripping the prefix", () => {
    expect(() => parsePackages([{ name: "pip:" }])).toThrow(/resolves to an empty package/);
  });

  it("accepts registry package identifiers and exact ecosystem versions", () => {
    expect(
      parsePackages([
        { name: "npm:@aex/example-package", version: "1.2.3-beta.1+build.7" },
        { name: "pip:typing-extensions", version: "4.12.2" },
        { name: "apt:libssl3:amd64", version: "3.0.13-0ubuntu3.5" }
      ])
    ).toEqual([
      { name: "@aex/example-package", version: "1.2.3-beta.1+build.7", ecosystem: "npm" },
      { name: "typing-extensions", version: "4.12.2", ecosystem: "pip" },
      { name: "libssl3:amd64", version: "3.0.13-0ubuntu3.5", ecosystem: "apt" }
    ]);
  });

  it.each([
    [{ name: "apt:a" }, /valid apt registry package name/],
    [{ name: "apt:--allow-unauthenticated" }, /valid apt registry package name/],
    [{ name: "apt:../../tmp/package.deb" }, /valid apt registry package name/],
    [{ name: "npm:https://example.test/package.tgz" }, /valid npm registry package name/],
    [{ name: "npm:pkg", version: "^1.2.3" }, /exact npm version/],
    [{ name: "pip:-r" }, /valid pip registry package name/],
    [{ name: "pip:pkg", version: "@ https://example.test/pkg.whl" }, /exact pip version/]
  ])("rejects package-manager options, paths, URLs, and non-exact versions", (entry, expected) => {
    expect(() => parsePackages([entry])).toThrow(expected);
  });
});

describe("packageInstallString", () => {
  it("emits the ecosystem-correct version join", () => {
    expect(packageInstallString({ name: "pandas", version: "2.2.0", ecosystem: "pip" })).toBe("pandas==2.2.0");
    expect(packageInstallString({ name: "express", version: "4.18.0", ecosystem: "npm" })).toBe("express@4.18.0");
    expect(packageInstallString({ name: "ffmpeg", version: "7:6.1", ecosystem: "apt" })).toBe("ffmpeg=7:6.1");
  });

  it("emits the bare name when no version is given", () => {
    expect(packageInstallString({ name: "ffmpeg", ecosystem: "apt" })).toBe("ffmpeg");
  });

  it("revalidates parsed package objects before rendering manager input", () => {
    expect(() => packageInstallString({ name: "--pre", ecosystem: "pip" })).toThrow(/valid pip registry package name/);
    expect(() => packageInstallString({ name: "express", version: "latest", ecosystem: "npm" })).toThrow(
      /exact npm version/
    );
  });
});

