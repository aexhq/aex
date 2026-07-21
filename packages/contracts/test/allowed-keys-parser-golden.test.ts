import { describe, expect, it } from "vitest";
import {
  parseApprovalGate,
  parseInlineSecrets,
  parseResponseFormat,
  parseSessionLimits,
  parseSessionMachine,
  parseSessionSubmissionRequest,
  parseSessionWebhook,
  parseSubmission,
  redactSideEffectAuditMetadata,
  type PlatformSubmission
} from "../src/internal.js";
import {
  parseAssetRefFields,
  parseMcpServerRef,
  parseSessionRequestConfig
} from "../src/session-config.js";
import { parsePostHook } from "../src/post-hook.js";

function baseSubmission(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    model: "claude-haiku-4-5",
    prompt: ["hello"],
    assets: { files: [], skills: [], tools: [], instructions: [] },
    mcpServers: [],
    builtinTools: "default",
    ...overrides
  };
}

function baseRequest(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    workspaceId: "workspace-1",
    idempotencyKey: "key-1",
    provider: "anthropic",
    submission: baseSubmission(),
    secrets: { apiKeys: { anthropic: "test-provider-key" } },
    ...overrides
  };
}

describe("strict parser key compatibility", () => {
  const cases: readonly [string, () => unknown, string][] = [
    [
      "environment",
      () => parseSubmission(baseSubmission({ environment: { unknown: true } })),
      "submission.environment.unknown is not an allowed field; permitted: networking, packages, envVars"
    ],
    [
      "networking",
      () => parseSubmission(baseSubmission({ environment: { networking: { unknown: true } } })),
      "submission.environment.networking.unknown is not an allowed field; permitted: mode, allowedHosts"
    ],
    [
      "package",
      () => parseSubmission(baseSubmission({ environment: { packages: [{ unknown: true }] } })),
      "submission.environment.packages[0].unknown is not an allowed field; permitted: name, version"
    ],
    [
      "inline secrets",
      () => parseInlineSecrets({ unknown: true }),
      "secrets.unknown is not an allowed field; permitted: apiKeys, mcpServers, envSecrets"
    ],
    [
      "MCP secret",
      () => parseInlineSecrets({ mcpServers: [{ unknown: true }] }),
      "secrets.mcpServers[0].unknown is not an allowed field; permitted: name, url, headers"
    ],
    [
      "top-level submission request",
      () => parseSessionSubmissionRequest(baseRequest({ unknown: true })),
      "submission.unknown is not an allowed field; permitted: workspaceId, idempotencyKey, provider, submission, runtimeSize, runtimeKind, timeout, webhook, limits, machine, secrets"
    ],
    [
      "webhook",
      () => parseSessionWebhook({ unknown: true }),
      "webhook.unknown is not an allowed field; permitted: url"
    ],
    [
      "limits",
      () => parseSessionLimits({ unknown: true }),
      "limits.unknown is not an allowed field; permitted: maxConcurrentChildSessions, maxSubagentDepth, maxSpendUsd, maxTurns, maxStepsPerTurn"
    ],
    [
      "machine",
      () => parseSessionMachine({ unknown: true }),
      "machine.unknown is not an allowed field; permitted: spot"
    ],
    [
      "submission body",
      () => parseSubmission({ unknown: true }),
      "submission.unknown is not an allowed field; permitted: model, system, prompt, assets, mcpServers, secretEnv, environment, securityProfile, metadata, fileCapture, builtinTools, outputMode, responseFormat, approvalGate, platform"
    ],
    [
      "assets",
      () => parseSubmission(baseSubmission({ assets: { unknown: true } })),
      "submission.assets.unknown is not allowed; permitted: files, skills, tools, instructions"
    ],
    [
      "workspace resource",
      () => parseSubmission(baseSubmission({ assets: { files: [{ unknown: true }] } })),
      "submission.assets.files[0].unknown is not allowed"
    ],
    [
      "platform injection",
      () => parseSubmission(baseSubmission({ platform: { unknown: true } })),
      "submission.platform.unknown is not an allowed field; permitted: systemPrompt"
    ],
    [
      "text response format",
      () => parseResponseFormat({ kind: "text", unknown: true }),
      "submission.responseFormat.unknown is not allowed when kind is 'text'"
    ],
    [
      "JSON-schema response format",
      () => parseResponseFormat({ kind: "json_schema", unknown: true }),
      "submission.responseFormat.unknown is not an allowed field; permitted: kind, schema, strict, name"
    ],
    [
      "approval gate",
      () => parseApprovalGate({ unknown: true }),
      "submission.approvalGate.unknown is not an allowed field; permitted: tools"
    ],
    [
      "file capture",
      () => parseSubmission(baseSubmission({ fileCapture: { unknown: true } })),
      "submission.fileCapture.unknown is not an allowed field; permitted: allowedDirs, deniedDirs, captureTimeoutMs, maxFileBytes, maxTotalBytes, maxFiles"
    ],
    [
      "asset ref",
      () => parseAssetRefFields({ unknown: true }, "asset"),
      "asset contains unexpected field for asset ref: unknown"
    ],
    [
      "MCP ref",
      () => parseMcpServerRef({ unknown: true }, "mcp"),
      "mcp.unknown is not an allowed field for McpServerRef; permitted: name, url, transport"
    ],
    [
      "session-config MCP ref",
      () => parseSessionRequestConfig({
        model: "claude-haiku-4-5",
        prompt: "hello",
        mcpServers: [{ unknown: true }]
      }),
      "session request config mcpServers[0].unknown is not an allowed field for SessionConfigMcpServer; permitted: name, url, transport, headers"
    ],
    [
      "session request config",
      () => parseSessionRequestConfig({ unknown: true }),
      "session request config contains unexpected field: unknown"
    ],
    [
      "post hook",
      () => parsePostHook({ unknown: true }, "submission.postHook"),
      "submission.postHook.unknown is not an allowed field; permitted: command, timeout, maxTurns, maxChars"
    ],
    [
      "audit status metadata",
      () => redactSideEffectAuditMetadata({ status: { unknown: true } } as never),
      "side-effect audit metadata.status.unknown is not supported"
    ],
    [
      "audit dimensions metadata",
      () => redactSideEffectAuditMetadata({ dimensions: { unknown: true } } as never),
      "side-effect audit metadata.dimensions.unknown is not supported"
    ],
    [
      "audit metadata",
      () => redactSideEffectAuditMetadata({ unknown: true } as never),
      "side-effect audit metadata.unknown is not supported"
    ],
    [
      "deletion audit metadata subset",
      () => redactSideEffectAuditMetadata({ dimensions: { provider: "anthropic" } }, "session.delete.completed"),
      "side-effect audit metadata.dimensions is not supported"
    ]
  ];

  it.each(cases)("preserves the exact %s unknown-key error", (_name, invoke, message) => {
    expect(invoke).toThrowError(new Error(message));
  });

  it("preserves first-key and reserved-secret error precedence", () => {
    expect(() => parseInlineSecrets({ unknown: true, __aex_internal: true })).toThrowError(
      "secrets.unknown is not an allowed field; permitted: apiKeys, mcpServers, envSecrets"
    );
    expect(() => parseInlineSecrets({ __aex_internal: true, unknown: true })).toThrowError(
      "secrets.__aex_internal uses the platform-internal __aex_ namespace and may not be set by callers"
    );
    expect(() => parseMcpServerRef({ command: "local", unknown: true }, "mcp")).toThrowError(
      "stdio MCP servers are not supported by Aex. Aex supports remote MCP servers over HTTP/SSE only."
    );
  });

  it("preserves parsed output order and empty-object normalization", () => {
    const parsed = parseSubmission(baseSubmission({
      system: "system",
      environment: {},
      fileCapture: {},
      approvalGate: { tools: [] },
      platform: {}
    })) as PlatformSubmission;

    expect(Object.keys(parsed)).toEqual([
      "model",
      "system",
      "prompt",
      "assets",
      "mcpServers",
      "builtinTools"
    ]);
    expect(parsed).not.toHaveProperty("environment");
    expect(parsed).not.toHaveProperty("fileCapture");
    expect(parsed).not.toHaveProperty("approvalGate");
    expect(parsed).not.toHaveProperty("platform");
    expect(parseInlineSecrets(null)).toEqual({});
    expect(parseSessionLimits({})).toBeUndefined();
    expect(parseSessionMachine({})).toBeUndefined();
    expect(parseApprovalGate({ tools: [] })).toBeUndefined();
    expect(parsePostHook({ command: "   " })).toBeUndefined();
  });
});
