/**
 * TypeScript consumer tests for the slim launch SDK surface.
 *
 * These compile a real installed `@aexhq/sdk` package from the perspective of
 * an app author. The test intentionally imports from the root package only.
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";

describe("typescript consumer", () => {
  let install: InstallResult;

  beforeAll(async () => {
    install = await installAex({ isolated: true });
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

  it("slim root SDK surface type-checks from clean NodeNext and Bundler consumers", async () => {
    const nodeNextConfig = {
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
      include: ["slim-consumer.ts", "slim-negative.ts"]
    };
    const bundlerConfig = {
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
      include: ["slim-consumer.ts", "slim-negative.ts"]
    };
    const consumer = `
      import {
        Aex,
        type Message,
        type RunResult,
        type SessionCreateOptions,
        type SessionRunOptions,
        type SessionRunResult
      } from "@aexhq/sdk";

      const fetchFake: typeof fetch = async () =>
        new Response(JSON.stringify({ ok: true }), {
          status: 200,
          headers: { "content-type": "application/json" }
        });

      const client = new Aex("aex_type_surface", {
        baseUrl: "https://api.example.invalid",
        fetch: fetchFake
      });
      const clientWithDefaults = new Aex("aex_type_surface");

      const createOptions = {
        model: "claude-haiku-4-5"
      } satisfies SessionCreateOptions;

      const runOptions = {
        model: "claude-haiku-4-5",
        message: "Summarize the session in one sentence."
      } satisfies SessionRunOptions;

      const session = await client.sessions.open("sess_type_surface");
      const messages: readonly Message[] = await session.messages.all();
      const renderedMessages: readonly string[] = messages.map((message) => {
        const role: Message["sender"] = message.sender;
        const text: string = message.text;
        return role + ": " + text;
      });

      const runResultPromise: Promise<RunResult> = client.run(runOptions);
      const sessionRunPromise: Promise<SessionRunResult> = client.sessions.run(runOptions);
      const directTextPromise: Promise<string | undefined> = (async () => (await runResultPromise).text)();
      const directRunMessagesPromise: Promise<readonly Message[]> = (async () => (await runResultPromise).messages)();
      const directSessionTextPromise: Promise<string | undefined> = (async () => (await sessionRunPromise).text)();
      const directSessionMessagesPromise: Promise<readonly Message[]> = (async () => (await sessionRunPromise).messages)();

      void clientWithDefaults;
      void createOptions;
      void renderedMessages;
      void directTextPromise;
      void directRunMessagesPromise;
      void directSessionTextPromise;
      void directSessionMessagesPromise;
    `;
    const negative = `
      import type { SessionCreateOptions, SessionRunOptions } from "@aexhq/sdk";

      // @ts-expect-error AgentExecutor is not part of the slim root surface.
      import { AgentExecutor } from "@aexhq/sdk";
      // @ts-expect-error ProxyEndpoint is not part of the slim root surface.
      import { ProxyEndpoint } from "@aexhq/sdk";
      // @ts-expect-error Data-source chat tools are not part of the slim root surface.
      import { createDataTools } from "@aexhq/sdk";
      // @ts-expect-error Data-source chat tools are not part of the slim root surface.
      import { createCorpusTools } from "@aexhq/sdk";
      // @ts-expect-error Data-source chat tools are not part of the slim root surface.
      import { DataToolError } from "@aexhq/sdk";
      // @ts-expect-error Data-source chat tools are not part of the slim root surface.
      import { DATA_TOOLS_INSTRUCTIONS } from "@aexhq/sdk";
      // @ts-expect-error Decode helpers are replaced by direct text/messages.
      import { decodeAssistantText } from "@aexhq/sdk";
      // @ts-expect-error Decode-style helpers are replaced by direct text/messages.
      import { decodeToolCalls } from "@aexhq/sdk";
      // @ts-expect-error Decode-style helpers are replaced by direct text/messages.
      import { summarizeRunTrace } from "@aexhq/sdk";
      // @ts-expect-error Decode-style helpers are replaced by direct text/messages.
      import { textOf } from "@aexhq/sdk";
      // @ts-expect-error DataTools is not part of the slim root surface.
      import type { DataTools } from "@aexhq/sdk";
      // @ts-expect-error Proxy wire types are not part of the slim root surface.
      import type { PlatformProxyEndpoint } from "@aexhq/sdk";

      const createOptions = {
        model: "claude-haiku-4-5",
        // @ts-expect-error proxyEndpoints is not a slim session create option.
        proxyEndpoints: []
      } satisfies SessionCreateOptions;

      const runOptions = {
        model: "claude-haiku-4-5",
        message: "hello",
        // @ts-expect-error proxyEndpoints is not a slim session run option.
        proxyEndpoints: []
      } satisfies SessionRunOptions;

      void createOptions;
      void runOptions;
      export {};
    `;

    writeFileSync(join(install.installDir, "slim-consumer.ts"), consumer);
    writeFileSync(join(install.installDir, "slim-negative.ts"), negative);
    writeFileSync(join(install.installDir, "tsconfig.nodenext.json"), JSON.stringify(nodeNextConfig, null, 2));
    writeFileSync(join(install.installDir, "tsconfig.bundler.json"), JSON.stringify(bundlerConfig, null, 2));

    await runTsc("tsconfig.nodenext.json");
    await runTsc("tsconfig.bundler.json");
  });
});
