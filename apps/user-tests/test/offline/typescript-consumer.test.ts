/**
 * Scenario 4: typescript-consumer.test.ts
 *
 * Catches broken .d.ts / export metadata. A real consumer project (and
 * any AI agent doing `tsc --noEmit`) must be able to import the SDK's
 * public root surface under real consumer module resolution and have
 * it type-check.
 *
 * Strategy:
 *   1. Install aex into the shared fixture tempdir.
 *   2. Add TypeScript as a devDependency in the same tempdir.
 *   3. Write consumer tsconfigs + sources that import values and types
 *      only from the root `aex` entry.
 *   4. Spawn `tsc --noEmit` from the install's local typescript.
 *   5. Assert exit 0 with no diagnostics.
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { installAex, runCommand, type InstallResult } from "../_fixtures/install.js";

const IS_WINDOWS = process.platform === "win32";

describe("typescript consumer", () => {
  let install: InstallResult;

  beforeAll(async () => {
    // Isolated: this scenario mutates the tree (npm-installs typescript +
    // @types/node and writes fixed-name consumer sources), so it must not
    // share the read-only per-worker install.
    install = await installAex({ isolated: true });
    // Add TypeScript + Node declarations to the same install tempdir.
    // The SDK is a Node package and its public declarations reference
    // node:* modules, so strict consumers need the matching type package.
    const npm = IS_WINDOWS ? "npm.cmd" : "npm";
    const result = await runCommand(
      npm,
      ["install", "typescript@5.8.3", "@types/node@20", "--no-audit", "--no-fund", "--ignore-scripts"],
      { cwd: install.installDir, timeoutMs: 120_000 }
    );
    if (result.exitCode !== 0) {
      throw new Error(`typescript install failed: ${result.stderr}`);
    }
  }, 240_000);

  afterAll(() => {
    install?.cleanup();
  });

  async function runTsc(projectFile: string): Promise<void> {
    const tscBin = join(install.installDir, "node_modules", ".bin", IS_WINDOWS ? "tsc.cmd" : "tsc");
    const result = await runCommand(tscBin, ["--noEmit", "-p", projectFile], {
      cwd: install.installDir,
      timeoutMs: 120_000
    });
    if (result.exitCode !== 0) {
      throw new Error(
        `tsc --noEmit -p ${projectFile} failed (exit ${result.exitCode}):\n` +
          `--- stdout ---\n${result.stdout}\n--- stderr ---\n${result.stderr}`
      );
    }
    expect(result.exitCode).toBe(0);
  }

  it("public root SDK surface type-checks from a clean NodeNext consumer", async () => {
    const tsconfig = {
      compilerOptions: {
        target: "ES2022",
        module: "NodeNext",
        moduleResolution: "NodeNext",
        strict: true,
        exactOptionalPropertyTypes: true,
        noUncheckedIndexedAccess: true,
        noEmit: true,
        skipLibCheck: false,
        lib: ["ES2022", "DOM", "DOM.Iterable"],
        types: ["node"]
      },
      include: ["consumer.ts", "legacy-negative.ts"]
    };
    const consumer = `
      import {
        AgentsMd,
        AexApiError,
        AgentExecutor,
        AexError,
        CleanupError,
        CredentialValidationError,
        DEFAULT_RUNTIME_SIZE,
        DEFAULT_RUN_PROVIDER,
        File,
        McpServer,
        ProviderError,
        ProxyEndpoint,
        RUN_PROVIDERS,
        RUNTIME_KINDS,
        RUNTIME_SIZES,
        RuntimeSizes,
        RuntimeValidationError,
        SecretString,
        Skill,
        buildPlatformAllowedHosts,
        bundleSkillFiles,
        collectManagedUnsupportedFeatures,
        hashSkillBundle,
        isRunStarted,
        isTextMessage,
        redactSecrets,
        selectRuntime,
        validateProxyAuth,
        type AgentsMdRef,
        type AnthropicSecrets,
        type McpServerSecret,
        type Output,
        type OutputFileSelector,
        type PlatformProxyEndpoint,
        type PlatformRunSubmissionRequest,
        type ProxyAuthShape,
        type ProxyAuthValue,
        type ProxyEndpointCommonOptions,
        type ProxyMethod,
        type ProxyResponseMode,
        type Run,
        type RunEvent,
        type RunProvider,
        type RuntimeResources,
        type RuntimeSize,
        type RuntimeKind,
        type RuntimeValidationCode,
        type SkillBundleManifest,
        type SkillFiles,
        type SkillRef,
        type SubmitRunOptions,
        type WaitForRunOptions
      } from "@aexhq/sdk";

      const provider: RunProvider = DEFAULT_RUN_PROVIDER;
      const runtime: RuntimeKind = RUNTIME_KINDS[0];
      const runtimeSize: RuntimeSize = RuntimeSizes.SHARED_2X_2GB;
      const defaultRuntimeSize: RuntimeSize = DEFAULT_RUNTIME_SIZE;
      const method: ProxyMethod = "GET";
      const responseMode: ProxyResponseMode = "headers_only";
      const authShape: ProxyAuthShape = { type: "header", name: "x-api-key" };
      const authValue: ProxyAuthValue = { type: "header", value: "proxy-test-value" };
      const resources: RuntimeResources = { cpus: 2, memoryMb: 2048 };
      const anthropicSecrets: AnthropicSecrets = { apiKey: "sk-ant-type-surface" };
      const mcpSecret: McpServerSecret = {
        name: "docs",
        url: "https://mcp.example.test/sse",
        headers: { authorization: "Bearer test" }
      };

      const commonProxy: ProxyEndpointCommonOptions = {
        name: "metadata",
        baseUrl: "https://example.test",
        allowMethods: [method],
        allowPathPrefixes: ["/v1"],
        allowHeaders: ["accept"],
        responseMode
      };
      const proxy = ProxyEndpoint.header({ ...commonProxy, header: "x-api-key", value: "secret" });
      const publicProxy = ProxyEndpoint.none({
        name: "public",
        baseUrl: "https://public.example.test",
        allowMethods: ["GET"],
        allowPathPrefixes: ["/"]
      });
      const mcp = McpServer.remote({
        name: "docs",
        url: "https://mcp.example.test/sse",
        transport: "sse",
        headers: { authorization: "Bearer test" }
      });
      const workspaceMcp = McpServer.fromId("mcp_abcdefgh12345678");
      const skillFiles = {
        "SKILL.md": "# Surface skill\\nUse the typed SDK surface."
      } satisfies SkillFiles;
      const skillBundle = bundleSkillFiles(skillFiles);
      const skillHash: Promise<string> = hashSkillBundle(skillBundle.zip);
      const inlineSkill = await Skill.fromFiles({ name: "surface-skill", files: skillFiles });
      const agentsMd = await AgentsMd.fromContent("# Rules\\nKeep outputs concise.", { name: "surface-rules" });
      const file = await File.fromBytes({
        name: "surface-file",
        bytes: new TextEncoder().encode("hello"),
        mountPath: "/mnt/session/surface.txt"
      });

      const anthropicOptions = {
        provider: "anthropic",
        runtime: "managed",
        model: "claude-haiku-4-5",
        system: "Be precise.",
        prompt: ["Read the attached file.", "Reply with a short acknowledgement."],
        skills: [inlineSkill],
        agentsMd: [agentsMd],
        files: [file],
        mcpServers: [mcp, workspaceMcp],
        proxyEndpoints: [proxy, publicProxy],
        outputs: { allowedDirs: ["/workspace/outputs"] },
        builtins: ["developer"],
        environment: {
          networking: { mode: "limited", allowedHosts: ["example.test"] },
          packages: [{ ecosystem: "apt", name: "jq" }],
          envVars: { USER_SURFACE_TEST: "1" }
        },
        metadata: { suite: "typescript-consumer", runtime: "managed" },
        runtimeSize,
        timeout: "15m",
        secrets: {
          anthropic: anthropicSecrets,
          mcpServers: [mcpSecret],
          proxyEndpointAuth: [{ name: "metadata", value: authValue }]
        },
        idempotencyKey: "type-surface-anthropic-managed"
      } satisfies SubmitRunOptions;

      const managedOptions = {
        provider: "deepseek",
        runtime: "managed",
        model: "deepseek-chat",
        prompt: "Say hello.",
        runtimeSize: defaultRuntimeSize,
        builtins: [],
        secrets: { deepseek: { apiKey: "sk-deepseek-type-surface" } },
        idempotencyKey: "type-surface-managed"
      } satisfies SubmitRunOptions;

      const wireRequest = {
        workspaceId: "ws_type_surface",
        idempotencyKey: "wire-type-surface",
        credentialMode: "byok",
        provider,
        runtime,
        submission: {
          model: "claude-haiku-4-5",
          system: "Be precise.",
          prompt: ["hello"],
          skills: [{ kind: "provider", vendor: "anthropic", skillId: "pdf" }],
          agentsMd: [],
          files: [],
          mcpServers: [],
          environment: { envVars: { USER_SURFACE_TEST: "1" } },
          metadata: { surface: "root" },
          outputs: { allowedDirs: ["/workspace/outputs"] },
          builtins: ["developer"]
        },
        secrets: { anthropic: anthropicSecrets },
        proxyEndpoints: [proxy.declaration],
        runtimeSize,
        timeoutMs: 15 * 60_000
      } satisfies PlatformRunSubmissionRequest;

      const selectedRuntime: RuntimeKind = selectRuntime(wireRequest);
      const managedUnsupportedFeatures: string[] = collectManagedUnsupportedFeatures(wireRequest);
      const validationCode: RuntimeValidationCode = "feature_runtime_mismatch";
      const platformEndpoint: PlatformProxyEndpoint = proxy.declaration;
      const skillRef: SkillRef = inlineSkill.ref as SkillRef;
      const agentsRef = agentsMd.ref as AgentsMdRef;
      const manifest = { schemaVersion: "1", files: [] } as unknown as SkillBundleManifest;
      const outputSelector: OutputFileSelector = { path: "report.txt", match: "suffix" };
      const waitOpts: WaitForRunOptions = { intervalMs: 100, timeoutMs: 1_000 };

      const fetchFake = async () =>
        new Response(JSON.stringify({
          id: "run_type_surface",
          workspaceId: "ws_type_surface",
          status: "queued",
          provider: "anthropic",
          runtime: "managed",
          createdAt: new Date(0).toISOString()
        }), { status: 202, headers: { "content-type": "application/json" } });
      const client = new AgentExecutor({
        apiToken: "ant_type_surface",
        baseUrl: "https://example.invalid",
        fetch: fetchFake
      });
      const runIdPromise: Promise<string> = client.submitRun(anthropicOptions);
      const runPromise: Promise<Run> = client.getRun("run_type_surface");
      const eventsPromise: Promise<readonly RunEvent[]> = client.listEvents("run_type_surface");
      const outputsPromise: Promise<readonly Output[]> = client.outputs("run_type_surface");
      const downloadPromise: Promise<Uint8Array> = client.downloadOutput("run_type_surface", outputSelector);

      const errors = [
        AexError,
        AexApiError,
        CleanupError,
        CredentialValidationError,
        ProviderError,
        RuntimeValidationError
      ];
      const secret = new SecretString("sk-ant-type-surface", "anthropic api key");
      const redacted = redactSecrets({ secret: String(secret), nested: ["sk-ant-type-surface"] });
      const exportedFns = [
        buildPlatformAllowedHosts,
        validateProxyAuth,
        isRunStarted,
        isTextMessage
      ];

      void RUN_PROVIDERS;
      void RUNTIME_SIZES;
      void resources;
      void selectedRuntime;
      void managedUnsupportedFeatures;
      void validationCode;
      void platformEndpoint;
      void skillRef;
      void agentsRef;
      void manifest;
      void anthropicOptions;
      void managedOptions;
      void skillHash;
      void runIdPromise;
      void runPromise;
      void eventsPromise;
      void outputsPromise;
      void downloadPromise;
      void errors;
      void redacted;
      void exportedFns;
    `;
    const legacyNegative = `
      // @ts-expect-error legacy SDK client class must stay absent from the root surface
      import { AexClient } from "@aexhq/sdk";
      // @ts-expect-error legacy platform class must stay absent from the root surface
      import { AexPlatformClient } from "@aexhq/sdk";
      // @ts-expect-error legacy template class must stay absent from the root surface
      import { Template } from "@aexhq/sdk";
      // @ts-expect-error legacy template type must stay absent from the root surface
      import type { TemplateDefinition } from "@aexhq/sdk";
      // @ts-expect-error legacy blueprint type must stay absent from the root surface
      import type { Blueprint } from "@aexhq/sdk";
      // @ts-expect-error legacy compile helper must stay absent from the root surface
      import { compileTemplate } from "@aexhq/sdk";
      // @ts-expect-error legacy run reference must stay absent from the root surface
      import type { RunRef } from "@aexhq/sdk";
      // @ts-expect-error removed debug-log result type must stay absent from the root surface
      import type { RunDebugLogs } from "@aexhq/sdk";
      import { AgentExecutor } from "@aexhq/sdk";
      const client = new AgentExecutor({ apiToken: "ant_legacy_negative", baseUrl: "https://example.invalid" });
      // @ts-expect-error removed logs download helper must stay absent from AgentExecutor
      void client.downloadLogs("run_type_surface");
      // @ts-expect-error removed debug logs helper must stay absent from AgentExecutor
      void client.getRunDebugLogs("run_type_surface");
      // @ts-expect-error removed debug logs alias must stay absent from AgentExecutor
      void client.debugLogs("run_type_surface");
      export {};
    `;
    writeFileSync(join(install.installDir, "tsconfig.json"), JSON.stringify(tsconfig, null, 2));
    writeFileSync(join(install.installDir, "consumer.ts"), consumer);
    writeFileSync(join(install.installDir, "legacy-negative.ts"), legacyNegative);

    await runTsc("tsconfig.json");
  });

  it("public root SDK surface type-checks from a clean Bundler consumer", async () => {
    const tsconfig = {
      compilerOptions: {
        target: "ES2022",
        module: "ESNext",
        moduleResolution: "Bundler",
        strict: true,
        exactOptionalPropertyTypes: true,
        noUncheckedIndexedAccess: true,
        noEmit: true,
        skipLibCheck: false,
        verbatimModuleSyntax: true,
        lib: ["ES2022", "DOM", "DOM.Iterable"],
        types: ["node"]
      },
      include: ["bundler-consumer.ts"]
    };
    const consumer = `
      import {
        AgentExecutor,
        RuntimeSizes,
        ProxyEndpoint,
        RUN_PROVIDERS,
        RUNTIME_KINDS,
        type RunProvider,
        type RuntimeKind,
        type RuntimeSize,
        type SubmitRunOptions
      } from "@aexhq/sdk";

      const provider: RunProvider = RUN_PROVIDERS[0];
      const runtime: RuntimeKind = RUNTIME_KINDS[0];
      const runtimeSize: RuntimeSize = RuntimeSizes.SHARED_1X_512MB;
      const proxy = ProxyEndpoint.bearer({
        name: "catalog",
        baseUrl: "https://example.test",
        token: "proxy-token",
        allowMethods: ["GET"],
        allowPathPrefixes: ["/v1"],
        responseMode: "status_only"
      });

      const options = {
        provider,
        runtime,
        model: "claude-haiku-4-5",
        prompt: "hello",
        proxyEndpoints: [proxy],
        runtimeSize,
        secrets: { anthropic: { apiKey: "sk-ant-bundler" } }
      } satisfies SubmitRunOptions;

      const client = new AgentExecutor({ apiToken: "ant_bundler", baseUrl: "https://example.invalid" });
      void client;
      void options;
    `;

    writeFileSync(join(install.installDir, "tsconfig.bundler.json"), JSON.stringify(tsconfig, null, 2));
    writeFileSync(join(install.installDir, "bundler-consumer.ts"), consumer);

    await runTsc("tsconfig.bundler.json");
  });
});
