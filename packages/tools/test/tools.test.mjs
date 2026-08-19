import assert from "node:assert/strict";
import test from "node:test";

import { Aex } from "@aexhq/sdk";
import { computer, subagents, webFetch, webSearch } from "../dist/index.js";

test("subagents selects the stable task primitive", () => {
  assert.deepEqual(subagents(), { kind: "aex.builtin", name: "task" });
  assert.ok(Object.isFrozen(subagents()));
});

test("computer expands to the ordered hand toolset", () => {
  const selected = computer();
  assert.deepEqual(
    selected.tools.map((tool) => tool.name),
    ["bash", "read", "write", "edit", "glob", "grep", "ls"],
  );
  assert.ok(Object.isFrozen(selected));
  assert.ok(Object.isFrozen(selected.tools));
});

test("managed web helpers select only their matching builtins", () => {
  assert.equal(webSearch().name, "web_search");
  assert.equal(webFetch().name, "web_fetch");
});

test("SDK creation compiles imported values into the sealed builtin order", async () => {
  let body;
  const aex = new Aex({
    apiKey: "aex_sk_test",
    fetch: async (_input, init) => {
      body = JSON.parse(init.body);
      return Response.json({
        id: "ses_01",
        object: "session",
        state: "idle",
        model: { provider: "anthropic", name: "test" },
        hand: { state: "ready", shape: "1gb" },
        storage: { workspace_bytes: 0, suspended_bytes: 0, artifact_bytes: 0 },
        created_at: "2026-08-19T10:00:00.000Z",
        updated_at: "2026-08-19T10:00:00.000Z",
        turns: 0,
        metadata: {},
      });
    },
  });

  await aex.sessions.create({
    model: { provider: "anthropic", name: "test", apiKey: "sk-ant-test" },
    tools: [subagents(), computer()],
  });

  assert.deepEqual(body.tools.builtin, [
    "task",
    "bash",
    "read",
    "write",
    "edit",
    "glob",
    "grep",
    "ls",
  ]);
});
