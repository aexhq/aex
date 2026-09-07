import test from "node:test";
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { createHash } from "node:crypto";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { mkdtemp, readFile, stat, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { login } from "../src/login.mjs";
import { saveSession, readSession, removeSession } from "../src/config.mjs";

const exec = promisify(execFile), bin = fileURLToPath(new URL("../src/index.mjs", import.meta.url));
async function serve(handler) {
  const server = createServer(handler);
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  return { url: `http://127.0.0.1:${server.address().port}`, close: () => new Promise(resolve => server.close(resolve)) };
}
test("CLI commands call shared account routes with the account credential", async () => {
  const requests = [];
  const api = await serve(async (req, res) => {
    let body = ""; for await (const chunk of req) body += chunk;
    requests.push({ path: req.url, method: req.method, token: req.headers.authorization, body: body ? JSON.parse(body) : undefined });
    if (req.method === "DELETE") res.writeHead(204).end();
    else res.setHeader("content-type", "application/json").end(JSON.stringify({ billing: "preview_customer_model_keys" }));
  });
  try {
    const cases = [
      [["account"], "GET", "/v1/account"], [["billing"], "GET", "/v1/account"], [["usage"], "GET", "/v1/usage"],
      [["keys", "list"], "GET", "/v1/keys"], [["keys", "create", "my key"], "POST", "/v1/keys", { name: "my key" }],
      [["keys", "rename", "key_one", "new name"], "PATCH", "/v1/keys/key_one", { name: "new name" }],
      [["keys", "revoke", "key_one"], "DELETE", "/v1/keys/key_one"], [["logout"], "DELETE", "/v1/account/session"],
    ];
    for (const [args, method, path, body] of cases) {
      const { stdout } = await exec(process.execPath, [bin, ...args], { env: { ...process.env, AEX_ACCOUNT_TOKEN: "aex_account_fixture", AEX_API_URL: api.url } });
      assert.doesNotThrow(() => JSON.parse(stdout));
      assert.deepEqual(requests.at(-1), { method, path, body, token: "Bearer aex_account_fixture" });
    }
    await assert.rejects(exec(process.execPath, [bin, "keys", "typo"]), /Unknown command/);
    const docs = await exec(process.execPath, [bin, "docs", "--no-browser"]);
    assert.equal(docs.stdout.trim(), "https://aex.dev/docs");
  } finally { await api.close(); }
});

test("browser login validates state and exchanges PKCE over the public API", async () => {
  let request;
  const api = await serve(async (req, res) => {
    let body = ""; for await (const chunk of req) body += chunk;
    assert.equal(req.url, "/v1/auth/exchange"); assert.equal(req.headers.authorization, undefined);
    request = JSON.parse(body);
    res.setHeader("content-type", "application/json").end(JSON.stringify({ token: "aex_account_fixture", expires: 2000000000 }));
  });
  let challenge, callbackResponse;
  try {
    const session = await login({ apiUrl: api.url, siteUrl: "https://aex.dev", onReady: async value => {
      const url = new URL(value); assert.equal(url.origin, "https://aex.dev"); assert.equal(url.pathname, "/cli");
      challenge = url.searchParams.get("code_challenge");
      const callback = new URL(url.searchParams.get("redirect_uri"));
      callback.search = new URLSearchParams({ state: "wrong", code: "once" });
      assert.equal((await fetch(callback)).status, 400);
      callback.searchParams.set("state", url.searchParams.get("state"));
      callbackResponse = fetch(callback).then(async response => { assert.equal(response.status, 200); await response.text(); });
    } });
    await callbackResponse;
    assert.equal(session.token, "aex_account_fixture");
    assert.equal(request.code, "once");
    assert.equal(createHash("sha256").update(request.code_verifier).digest("base64url"), challenge);
    await assert.rejects(login({ apiUrl: api.url, siteUrl: "https://aex.dev", onReady: () => {}, signal: AbortSignal.timeout(20) }), /timed out/);
  } finally { await api.close(); }
});

test("stored credentials round trip privately and cannot be sent to another origin", async () => {
  const dir = await mkdtemp(join(tmpdir(), "aex-cli-")); process.env.AEX_CONFIG_DIR = dir;
  try {
    const session = { token: "aex_account_fixture", expires: 2000000000, apiUrl: "https://api.aex.dev" };
    await saveSession(session); assert.deepEqual(await readSession(), session);
    assert.equal(JSON.parse(await readFile(join(dir, "account.json"), "utf8")).token, session.token);
    if (process.platform !== "win32") assert.equal((await stat(join(dir, "account.json"))).mode & 0o777, 0o600);
    await assert.rejects(exec(process.execPath, [bin, "account", "--api-url", "https://other.example"], { env: { ...process.env, AEX_ACCOUNT_TOKEN: "", AEX_API_URL: "" } }), /different API origin/);
    await removeSession(); await assert.rejects(readSession(), /Sign in first/);
  } finally { delete process.env.AEX_CONFIG_DIR; await rm(dir, { recursive: true }); }
});
