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
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";

describe("typescript consumer", () => {
  let install: InstallResult;

  beforeAll(async () => {
    // Isolated: this scenario mutates the tree (installs typescript +
    // @types/node and writes fixed-name consumer sources), so it must not
    // share the read-only per-worker install.
    install = await installAex({ isolated: true });
    // Add TypeScript + Node declarations to the same install tempdir.
    // The SDK's public declarations reference node:* modules, so strict
    // TypeScript consumers need the matching type package.
    const result = await runCommand(
      getBunCommand(),
      ["install", "typescript@5.8.3", "@types/node@20", "--ignore-scripts", "--no-progress"],
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
    const result = await runCommand(getBunCommand(), ["run", "tsc", "--noEmit", "-p", projectFile], {
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
        BUILTIN_TOOL_NAMES,
        BuiltinTools,
        CleanupError,
        CredentialValidationError,
        DEFAULT_BUILTIN_TOOLS,
        DEFAULT_RUNTIME_SIZE,
        DEFAULT_RUN_PROVIDER,
        File,
        McpServer,
        ProviderError,
        ProxyEndpoint,
        RUN_PROVIDERS,
        RUNTIME_SIZES,
        Models,
        Sizes,
        Secret,
        SecretString,
        Skill,
        buildPlatformAllowedHosts,
        bundleSkillFiles,
        hashSkillBundle,
        isRunStarted,
        isTextMessage,
        redactSecrets,
        textOf,
        validateProxyAuth,
        type AgentsMdRef,
        type BuiltinToolName,
        type InlineSecrets,
        type McpServerSecret,
        type Output,
        type OutputLink,
        type OutputLinkOptions,
        type OutputQuery,
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
        type RunResult,
        type RunProvider,
        type SessionCreateOptions,
        type SessionRunOptions,
        type SessionRunResult,
        type SessionTurnResult,
        type TextMessageRunEvent,
        type RuntimeResources,
        type RuntimeSize,
        type SkillBundleManifest,
        type SkillFiles,
        type SkillRef,
        type WaitForRunOptions
      } from "@aexhq/sdk";

      const provider: RunProvider = DEFAULT_RUN_PROVIDER;
      const runtimeSize: RuntimeSize = Sizes.SHARED_2X_8GB;
      const defaultRuntimeSize: RuntimeSize = DEFAULT_RUNTIME_SIZE;
      const explicitBuiltin: BuiltinToolName = BuiltinTools.web_fetch;
      const everyBuiltin: readonly BuiltinToolName[] = BUILTIN_TOOL_NAMES;
      const method: ProxyMethod = "GET";
      const responseMode: ProxyResponseMode = "headers_only";
      const authShape: ProxyAuthShape = { type: "header", name: "x-api-key" };
      const authValue: ProxyAuthValue = { type: "header", value: "proxy-test-value" };
      const resources: RuntimeResources = { cpus: 2, memoryMb: 2048 };
      const anthropicSecrets: InlineSecrets = { apiKeys: { anthropic: "sk-ant-type-surface" } };
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
        model: Models.CLAUDE_HAIKU_4_5,
        system: "Be precise.",
        message: ["Read the attached file.", "Reply with a short acknowledgement."],
        skills: [inlineSkill],
        agentsMd: [agentsMd],
        files: [file],
        mcpServers: [mcp, workspaceMcp],
        proxyEndpoints: [proxy, publicProxy],
        outputs: { allowedDirs: ["/workspace/outputs"] },
        // Default builtin set ON, plus the opt-in notebook tool (a builtin ref).
        includeBuiltinTools: true,
        tools: [BuiltinTools.notebook_edit],
        environment: {
          networking: { mode: "limited", allowedHosts: ["example.test"] },
          packages: [{ name: "apt:jq" }],
          variables: { USER_SURFACE_TEST: "1" },
          secrets: { SESSION_TOKEN: Secret.value("session-token") }
        },
        metadata: { suite: "typescript-consumer" },
        runtime: runtimeSize,
        overrides: { timeout: "15m" },
        apiKeys: { anthropic: "sk-ant-type-surface" },
        idempotencyKey: "type-surface-anthropic-managed"
      } satisfies SessionRunOptions;

      const managedOptions = {
        provider: "deepseek",
        model: Models.DEEPSEEK_V4_FLASH,
        runtime: defaultRuntimeSize,
        includeBuiltinTools: false,
        apiKeys: { deepseek: "sk-deepseek-type-surface" },
        idempotencyKey: "type-surface-managed"
      } satisfies SessionCreateOptions;

      const apiKeysOptions = {
        model: Models.CLAUDE_HAIKU_4_5,
        message: "hello",
        apiKeys: { anthropic: "sk-ant", openai: "sk-oai" }
      } satisfies SessionRunOptions;

      const wireRequest = {
        workspaceId: "ws_type_surface",
        idempotencyKey: "wire-type-surface",
        provider,
        submission: {
          model: Models.CLAUDE_HAIKU_4_5,
          system: "Be precise.",
          prompt: ["hello"],
          skills: [inlineSkill.ref as SkillRef],
          agentsMd: [],
          files: [],
          mcpServers: [],
          environment: { envVars: { USER_SURFACE_TEST: "1" } },
          metadata: { surface: "root" },
          outputs: { allowedDirs: ["/workspace/outputs"] },
          includeBuiltinTools: false,
          // Canonical (post-parse) shape: parseTools extracts the cherry-picked
          // builtin NAMES out of the wire \`tools\` union into \`builtinTools\`,
          // leaving \`tools\` to carry only custom ToolRef bundles. The bare-string
          // \`tools\` union (the pre-parse INPUT surface) is exercised by the
          // SubmitOptions block above.
          builtinTools: [BuiltinTools.web_search, explicitBuiltin, BuiltinTools.read_file, BuiltinTools.edit_file]
        },
        secrets: anthropicSecrets,
        proxyEndpoints: [proxy.declaration],
        runtimeSize,
        timeoutMs: 15 * 60_000
      } satisfies PlatformRunSubmissionRequest;

      const platformEndpoint: PlatformProxyEndpoint = proxy.declaration;
      const builtinCount: number = everyBuiltin.length;
      const skillRef: SkillRef = inlineSkill.ref as SkillRef;
      const agentsRef = agentsMd.ref as AgentsMdRef;
      const manifest = { schemaVersion: "1", files: [] } as unknown as SkillBundleManifest;
      const outputSelector: OutputFileSelector = { path: "report.txt", match: "suffix" };
      const outputQuery: OutputQuery = { dir: "reports", extension: ".json", type: "json" };
      const outputLinkOptions: OutputLinkOptions = { expiresIn: "15m" };
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
      // run() returns a settle-consistent RunResult; runAndCollect is its alias.
      const runResultPromise: Promise<RunResult> = client.run(apiKeysOptions);
      const collectPromise: Promise<RunResult> = client.runAndCollect(apiKeysOptions, {
        throwOnFailure: false,
        timeoutMs: 1_000
      });
      const sessionOptions = {
        model: Models.CLAUDE_HAIKU_4_5,
        system: "Be precise.",
        runtime: Sizes.SHARED_0_25X_1GB,
        overrides: { idleTtl: "3m" },
        environment: {
          variables: { USER_SURFACE_TEST: "1" },
          secrets: { SESSION_TOKEN: Secret.value("session-secret") }
        },
        apiKeys: { anthropic: "sk-ant-session" }
      } satisfies SessionCreateOptions;
      const sessionPromise = client.openSession(sessionOptions);
      const reopenedPromise = client.openSession("run_type_surface");
      const sessionRunPromise: Promise<SessionRunResult> = client.sessions.run({
        ...sessionOptions,
        message: "hello"
      });

      // The run-addressed reads (events / outputs / links / download / webhooks)
      // folded onto the SessionHandle — a session id doubles as the run handle.
      const runViewPromise: Promise<Run> = (async () => (await client.run(apiKeysOptions)).run)();
      const sessionTurnPromise: Promise<SessionTurnResult> = (async () => {
        const session = await client.sessions.open("run_type_surface");
        const outputsPromise: Promise<readonly Output[]> = session.outputs().list();
        const foundOutputsPromise: Promise<readonly Output[]> = session.outputs().find(outputQuery);
        const foundOutputPromise: Promise<Output | null> = session.outputs().findOne(outputQuery);
        const outputLinkPromise: Promise<OutputLink> = session.outputs().link(outputQuery, outputLinkOptions);
        const selectorLinkPromise: Promise<OutputLink> = session.outputs().link(outputSelector);
        const fetchOutputPromise: Promise<Response> = session.outputs().fetch({ filename: "report.json" });
        const eventArchiveLinkPromise: Promise<OutputLink> = session.events().archiveLink({ expiresIn: "1h" });
        const downloadPromise: Promise<Uint8Array> = session.outputs().download(outputSelector);
        const wholeArchivePromise: Promise<Uint8Array> = session.download();
        void session.events().list();
        void session.outputs().read(outputSelector);
        void session.wait(waitOpts);
        void session.unit();
        void session.events().stream();
        void session.events().streamEnvelopes();
        void session.webhooks().list();
        void outputsPromise;
        void foundOutputsPromise;
        void foundOutputPromise;
        void outputLinkPromise;
        void selectorLinkPromise;
        void fetchOutputPromise;
        void eventArchiveLinkPromise;
        void downloadPromise;
        void wholeArchivePromise;
        return await session.send("continue").done();
      })();
      const finalTextPromise: Promise<string> = (async () => {
        const result = await client.run(apiKeysOptions);
        let acc = "";
        for (const ev of result.events) {
          if (isTextMessage(ev)) {
            // Inside the guard, ev.data.text is narrowed to a string.
            const narrowed: TextMessageRunEvent = ev;
            acc += narrowed.data.text;
          }
        }
        return textOf(result.events) + acc;
      })();

      const errors = [
        AexError,
        AexApiError,
        CleanupError,
        CredentialValidationError,
        ProviderError,
        ProviderError
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
      void Models;
      void resources;
      void platformEndpoint;
      void builtinCount;
      void skillRef;
      void agentsRef;
      void manifest;
      void anthropicOptions;
      void managedOptions;
      void skillHash;
      void runViewPromise;
      void runResultPromise;
      void collectPromise;
      void sessionOptions;
      void sessionPromise;
      void reopenedPromise;
      void sessionRunPromise;
      void sessionTurnPromise;
      void finalTextPromise;
      void apiKeysOptions;
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
      // @ts-expect-error removed submit options type must stay absent from the root surface
      import type { SubmitOptions } from "@aexhq/sdk";
      // @ts-expect-error legacy runtime-sizes symbol was renamed to Sizes
      import { RuntimeSizes } from "@aexhq/sdk";
      // @ts-expect-error removed run-list page type must stay absent from the root surface
      import type { RunListPage } from "@aexhq/sdk";
      // @ts-expect-error removed run-list query type must stay absent from the root surface
      import type { RunListQuery } from "@aexhq/sdk";
      // @ts-expect-error removed run-summary type must stay absent from the root surface
      import type { RunSummary } from "@aexhq/sdk";
      import { AgentExecutor } from "@aexhq/sdk";
      const client = new AgentExecutor({ apiToken: "ant_legacy_negative", baseUrl: "https://example.invalid" });
      // @ts-expect-error removed logs download helper must stay absent from AgentExecutor
      void client.downloadLogs("run_type_surface");
      // @ts-expect-error removed debug logs helper must stay absent from AgentExecutor
      void client.getRunDebugLogs("run_type_surface");
      // @ts-expect-error removed debug logs alias must stay absent from AgentExecutor
      void client.debugLogs("run_type_surface");
      // The one-shot/run-addressed client surface folded into sessions — these
      // must all stay absent from the client (they live on SessionHandle now).
      // @ts-expect-error removed submit verb must stay absent from AgentExecutor
      void client.submit({ model: "claude-haiku-4-5" });
      // @ts-expect-error removed getRun verb must stay absent from AgentExecutor
      void client.getRun("run_type_surface");
      // @ts-expect-error removed listRuns verb must stay absent from AgentExecutor
      void client.listRuns();
      // @ts-expect-error removed searchOutputs verb must stay absent from AgentExecutor
      void client.searchOutputs({});
      // @ts-expect-error removed cancel verb must stay absent from AgentExecutor
      void client.cancel("run_type_surface");
      // @ts-expect-error removed download verb must stay absent from AgentExecutor
      void client.download("run_type_surface");
      // @ts-expect-error removed listEvents verb must stay absent from AgentExecutor
      void client.listEvents("run_type_surface");
      // @ts-expect-error removed wait verb must stay absent from AgentExecutor
      void client.wait("run_type_surface");
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
        Sizes,
        ProxyEndpoint,
        RUN_PROVIDERS,
        Models,
        type RunProvider,
        type RuntimeSize,
        type SessionCreateOptions
      } from "@aexhq/sdk";

      const provider: RunProvider = RUN_PROVIDERS[0];
      const runtimeSize: RuntimeSize = Sizes.SHARED_0_25X_1GB;
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
        model: Models.CLAUDE_HAIKU_4_5,
        proxyEndpoints: [proxy],
        runtime: runtimeSize,
        apiKeys: { anthropic: "sk-ant-bundler" }
      } satisfies SessionCreateOptions;

      const client = new AgentExecutor({ apiToken: "ant_bundler", baseUrl: "https://example.invalid" });
      void client;
      void options;
    `;

    writeFileSync(join(install.installDir, "tsconfig.bundler.json"), JSON.stringify(tsconfig, null, 2));
    writeFileSync(join(install.installDir, "bundler-consumer.ts"), consumer);

    await runTsc("tsconfig.bundler.json");
  });
});
