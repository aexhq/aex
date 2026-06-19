import { describe, expect, it } from "vitest";
import {
  getRunStatusKind,
  parseRunSubmissionRequest,
  isTerminalRunStatus,
  packageInstallString,
  RUN_TERMINAL_OUTCOMES,
  TERMINAL_RUN_STATUSES
} from "../src/index.js";

describe("platform status contracts", () => {
  it("classifies terminal and active run statuses", () => {
    expect(isTerminalRunStatus("succeeded")).toBe(true);
    expect(isTerminalRunStatus("cleanup_failed")).toBe(true);
    expect(isTerminalRunStatus("provider_running")).toBe(false);
    expect(getRunStatusKind("queued")).toBe("active");
    expect(getRunStatusKind("cleanup_failed")).toBe("terminal");
  });

  it("RUN_TERMINAL_OUTCOMES is exactly the four funnel write-outcomes", () => {
    expect(new Set(RUN_TERMINAL_OUTCOMES)).toEqual(
      new Set(["succeeded", "failed", "timed_out", "cancelled"])
    );
  });

  it("RUN_TERMINAL_OUTCOMES is a STRICT subset of the read-terminal set", () => {
    const readTerminal = new Set<string>(TERMINAL_RUN_STATUSES);
    for (const o of RUN_TERMINAL_OUTCOMES) {
      expect(readTerminal.has(o)).toBe(true);
    }
    // The read-terminal set additionally carries the post-terminal
    // housekeeping states the funnel never writes as an outcome.
    expect(TERMINAL_RUN_STATUSES.length).toBeGreaterThan(RUN_TERMINAL_OUTCOMES.length);
    expect(readTerminal.has("cleanup_failed")).toBe(true);
    expect(new Set<string>(RUN_TERMINAL_OUTCOMES).has("cleanup_failed")).toBe(false);
  });
});

describe("platform run submission schema", () => {
  const baseSecrets = { apiKey: "sk-ant-test" } as const;
  const baseSubmission = {
    model: "claude-haiku-4-5",
    prompt: ["say hello"],
    skills: [],
    agentsMd: [],
    files: [],
    mcpServers: []
  } as const;

  it("parses the minimal run submission contract", () => {
    const parsed = parseRunSubmissionRequest({
      workspaceId: "workspace-1",
      idempotencyKey: "idem-1",
      submission: { ...baseSubmission, metadata: { topic: "platform" } },
      secrets: baseSecrets
    });

    expect(parsed.provider).toBe("anthropic");
    expect(parsed.submission.prompt).toEqual(["say hello"]);
    expect(parsed.submission.metadata?.topic).toBe("platform");
    expect(parsed.secrets.apiKey).toBe("sk-ant-test");
  });

  it("accepts DeepSeek as an explicit provider with a flat apiKey", () => {
    const parsed = parseRunSubmissionRequest({
      provider: "deepseek",
      workspaceId: "workspace-1",
      idempotencyKey: "idem-1",
      submission: { ...baseSubmission, model: "deepseek-chat" },
      secrets: { apiKey: "sk-deepseek-test" }
    });

    expect(parsed.provider).toBe("deepseek");
    expect(parsed.secrets.apiKey).toBe("sk-deepseek-test");
  });

  it("rejects the removed cleanup policy field", () => {
    const base = {
      workspaceId: "workspace-1",
      idempotencyKey: "idem-1",
      submission: baseSubmission,
      secrets: baseSecrets
    };

    expect(parseRunSubmissionRequest(base).submission.prompt).toEqual(["say hello"]);
    expect(() => parseRunSubmissionRequest({ ...base, cleanup: { session: "delete" } })).toThrow(/cleanup/);
  });

  it("parses the platform.systemPrompt opt-out and rejects bad shapes", () => {
    const base = {
      workspaceId: "workspace-1",
      idempotencyKey: "idem-1",
      secrets: baseSecrets
    };

    // Omitted → field absent (default injection stays on downstream).
    expect(parseRunSubmissionRequest({ ...base, submission: baseSubmission }).submission.platform).toBeUndefined();

    // Explicit "off" survives parsing.
    expect(
      parseRunSubmissionRequest({
        ...base,
        submission: { ...baseSubmission, platform: { systemPrompt: "off" } }
      }).submission.platform
    ).toEqual({ systemPrompt: "off" });

    // Invalid enum value rejected.
    expect(() =>
      parseRunSubmissionRequest({
        ...base,
        submission: { ...baseSubmission, platform: { systemPrompt: "yes" } }
      })
    ).toThrow(/platform\.systemPrompt/);

    // Unknown nested key rejected.
    expect(() =>
      parseRunSubmissionRequest({
        ...base,
        submission: { ...baseSubmission, platform: { outputDir: "/x" } }
      })
    ).toThrow(/platform\.outputDir/);
  });

  it("rejects invalid submission shapes", () => {
    expect(() => parseRunSubmissionRequest({
      workspaceId: "workspace-1",
      submission: baseSubmission,
      secrets: baseSecrets
    })).toThrow(/idempotencyKey/);
  });

  it("requires a secrets block", () => {
    expect(() => parseRunSubmissionRequest({
      workspaceId: "workspace-1",
      idempotencyKey: "idem-1",
      submission: baseSubmission
    })).toThrow(/secrets/);
  });

  it("requires secrets.apiKey", () => {
    expect(() => parseRunSubmissionRequest({
      workspaceId: "workspace-1",
      idempotencyKey: "idem-1",
      submission: baseSubmission,
      secrets: { apiKey: "" }
    })).toThrow(/secrets\.apiKey must be a non-empty string/);
  });

  it("accepts mcpServers inside the secrets block", () => {
    const parsed = parseRunSubmissionRequest({
      workspaceId: "workspace-1",
      idempotencyKey: "idem-1",
      submission: {
        ...baseSubmission,
        mcpServers: [{ name: "files", url: "https://mcp.example.test" }]
      },
      secrets: {
        apiKey: "sk-ant-test",
        mcpServers: [{ name: "files", url: "https://mcp.example.test", headers: { authorization: "Bearer x" } }]
      }
    });

    expect(parsed.secrets.mcpServers?.[0]?.url).toBe("https://mcp.example.test");
  });

  it("accepts user tools as value-free asset refs", () => {
    const parsed = parseRunSubmissionRequest({
      workspaceId: "ws_123",
      idempotencyKey: "idem-tool",
      submission: {
        model: "claude-haiku-4-5",
        prompt: "use the lookup tool",
        skills: [],
        agentsMd: [],
        files: [],
        tools: [
          {
            kind: "asset",
            assetId: `asset_${"a".repeat(64)}`,
            name: "calendar_lookup",
            description: "Looks up calendar availability.",
            input_schema: {
              type: "object",
              properties: { start: { type: "string" } },
              required: ["start"]
            },
            entry: "index.js"
          }
        ],
        mcpServers: []
      },
      secrets: baseSecrets
    });

    expect(parsed.submission.tools).toEqual([
      {
        kind: "asset",
        assetId: `asset_${"a".repeat(64)}`,
        name: "calendar_lookup",
        description: "Looks up calendar availability.",
        input_schema: {
          type: "object",
          properties: { start: { type: "string" } },
          required: ["start"]
        },
        entry: "index.js"
      }
    ]);
  });

  it("rejects user tool names that collide with MCP namespace routing", () => {
    expect(() =>
      parseRunSubmissionRequest({
        workspaceId: "ws_123",
        idempotencyKey: "idem-tool",
        submission: {
          model: "claude-haiku-4-5",
          prompt: "use the lookup tool",
          skills: [],
          agentsMd: [],
          files: [],
          tools: [
            {
              kind: "asset",
              assetId: `asset_${"a".repeat(64)}`,
              name: "calendar__lookup",
              description: "Looks up calendar availability.",
              input_schema: { type: "object", properties: {}, required: [] },
              entry: "index.js"
            }
          ],
          mcpServers: []
        },
        secrets: baseSecrets
      })
    ).toThrow(/must not contain "__"/);
  });

  it("rejects stdio-shaped MCP servers with the canonical remote-only error", () => {
    const expected =
      "stdio MCP servers are not supported by Aex. Aex supports remote MCP servers over HTTP/SSE only.";

    expect(() => parseRunSubmissionRequest({
      workspaceId: "workspace-1",
      idempotencyKey: "idem-1",
      submission: {
        ...baseSubmission,
        mcpServers: [{ name: "local", transport: "stdio", command: "npx" }]
      },
      secrets: baseSecrets
    })).toThrow(expected);

    expect(() => parseRunSubmissionRequest({
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
    expect(() => parseRunSubmissionRequest({
      workspaceId: "workspace-1",
      idempotencyKey: "idem-1",
      submission: baseSubmission,
      secrets: { apiKey: "sk-ant-test", openai: { apiKey: "x" } }
    })).toThrow(
      /secrets\.openai is not an allowed field; permitted: apiKey, mcpServers, proxyEndpointAuth/
    );
  });

  it("rejects secret-bearing fields outside the secrets allowlist", () => {
    expect(() => parseRunSubmissionRequest({
      workspaceId: "workspace-1",
      idempotencyKey: "idem-1",
      providerApiKey: "sk-ant-test",
      submission: baseSubmission,
      secrets: baseSecrets
    })).toThrow(/providerApiKey/);
  });

  it("rejects a whitespace-only prompt that would trim to empty downstream", () => {
    const submit = (prompt: unknown) =>
      parseRunSubmissionRequest({
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
    secrets: { apiKey: "sk-ant-test" }
  } as const;
  const baseSubmission = {
    model: "claude-haiku-4-5",
    prompt: ["say hello"],
    skills: [],
    agentsMd: [],
    files: [],
    mcpServers: []
  } as const;

  const parsePackages = (packages: unknown) =>
    parseRunSubmissionRequest({
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
});

