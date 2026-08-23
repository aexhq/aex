import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { computer, defineEnvironment, linux } from "@aexhq/environment";
import { Aex } from "@aexhq/sdk";
import { bash, edit, glob, grep, ls, read, todo, write } from "../index.mjs";

const runtime = defineEnvironment({
  identity: "test.computer",
  protocol: "environment/v1",
  profile: computer({ platform: linux.amd64, network: "allowlist", recovery: "retained" }),
  serialize: () => ({}),
  handle: () => ({}),
});
const loop = Object.freeze({ source: "export const activate=()=>{}", sha256: "a".repeat(64), toolchain: "test-loop" });

test("official tools are prepared computer extensions with explicit requirements", () => {
  const values = [bash(), edit(), glob(), grep(), ls(), read(), todo(), write()];
  assert.deepEqual(values.map((value) => value.name), ["bash", "edit", "glob", "grep", "ls", "read", "todo", "write"]);
  for (const value of values) {
    assert.equal(value.kind, "aex.tool");
    assert.equal(value.requirements.workspace, true);
    assert.equal(value.requirements.recovery, "retained");
    assert.match(value.artifact.digest, /^[0-9a-f]{64}$/);
    assert.equal(value.artifact.target, "linux-amd64");
  }
  assert.equal(bash().requirements.processes, true);
  assert.equal(grep().requirements.processes, true);
});

test("the prepared runtime uses an immutable absolute entrypoint", async () => {
  const manifest = JSON.parse(await readFile(new URL("../dist/bash.artifact.json", import.meta.url), "utf8"));
  assert.equal(manifest.execute, `/artifacts/${manifest.digest}/execute`);
  const runtimeModule = (await import(new URL(`../dist/${manifest.blobs[0].file}`, import.meta.url))).default;
  assert.equal(runtimeModule.kind, "tool-runtime/v1");
  assert.equal(runtimeModule.name, "bash");
  assert.equal(typeof runtimeModule.execute, "function");
});

test("SDK creation binds every official tool to the one compatible environment", async () => {
  let body;
  const aex = new Aex({
    apiKey: "aex_sk_test",
    fetch: async (_input, init) => {
      body = JSON.parse(init.body);
      return Response.json({
        id: "ses_01",
        root_id: "ses_01",
        depth: 0,
        object: "session",
        state: "open",
        turn_state: "idle",
        model: { provider: "openai", name: "test", context_window_tokens: 32_768 },
        storage: { session_storage_bytes: 0, upload_reserved_bytes: 0 },
        created_at: "2026-08-23T10:00:00.000Z",
        updated_at: "2026-08-23T10:00:00.000Z",
        turns: 0,
        last_seq: 0,
        metadata: {},
      });
    },
  });
  const workspace = runtime();
  await aex.sessions.create({
    model: { provider: "openai", name: "test", apiKey: "sk-test" },
    loop,
    environments: { workspace },
    tools: [bash(), read(), write(), edit()],
  });

  assert.deepEqual(body.tools.items.map((item) => item.executor.environment), ["workspace", "workspace", "workspace", "workspace"]);
  assert.ok(body.tools.items.every((item) => item.executor.kind === "environment"));
  assert.equal(body.tool_bundles.length, 4);
});
