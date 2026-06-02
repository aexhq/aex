/**
 * USER TEST (SDK-driven) — `proxyEndpoints` round-trip on a managed run.
 *
 * Validates, through the installed SDK, that declaring a `ProxyEndpoint` on a
 * submission both MOUNTS the per-run proxy bridge file
 *   /mnt/session/uploads/antpath/index.json
 * AND ships the CONSUMER runtime bridge to the manifest's `antpath` path
 * (`/mnt/session/uploads/antpath/antpath`), so the agent can make a real proxy
 * round-trip. Uses a PUBLIC no-auth upstream (`ProxyEndpoint.none` →
 * httpbin.org) so no secret is involved; `responseMode: "full"` so the JSON
 * body surfaces to the agent.
 *
 * The runner downloads the pinned runtime bridge artifact by digest, verifies
 * sha256, writes /mnt/session/uploads/antpath/antpath, and the entrypoint drops
 * an `antpath` PATH wrapper.
 *
 * Only passes once the worker is deployed with ANTPATH_RUNTIME_BRIDGE_MANIFEST and
 * the dashboard ANTPATH_PROXY_PUBLIC_BASE_URL is set (the index.json's
 * `proxyBaseUrl` is composed from it). Runs on managed (deepseek). waitMs ~8min.
 */
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { installAntpath, type InstallResult } from "../_fixtures/install.js";
import { dense, requireUserEnv, runSdkScript, sdkRunnerScript } from "./_sdk.js";

const env = requireUserEnv({ deepseek: true });

describe("user/SDK: managed proxyEndpoints bridge round-trip succeeds", () => {
  let install: InstallResult;
  beforeAll(async () => {
    install = await installAntpath();
  }, 240_000);
  afterAll(() => install?.cleanup());

  it(
    "declaring a ProxyEndpoint mounts index.json + the runtime bridge and a real round-trip succeeds",
    async () => {
      const script = sdkRunnerScript({
        submit: `{
          provider: "deepseek",
          runtime: "managed",
          model: MODEL_DEEPSEEK,
          prompt: [
            "Using the shell, do EXACTLY the following and report short tokens:",
            "1) Check the proxy bridge file:",
            "   test -f /mnt/session/uploads/antpath/index.json && echo INDEX_PRESENT || echo INDEX_MISSING",
            "2) Make a real proxy round-trip with the antpath runtime bridge (invoke via node, which always works):",
            "   node /mnt/session/uploads/antpath/antpath proxy httpbin --path /get",
            "   (equivalently, bare 'antpath proxy httpbin --path /get' also works on this runtime)",
            "   If that command returns ANY HTTP/JSON response, echo PROXY_OK; if it errors, echo PROXY_ERR.",
            "Reply with ONLY the tokens you produced, separated by spaces."
          ],
          proxyEndpoints: [
            ProxyEndpoint.none({
              name: "httpbin",
              baseUrl: "https://httpbin.org",
              allowMethods: ["GET"],
              allowPathPrefixes: ["/"],
              responseMode: "full"
            })
          ],
          secrets: { deepseek: { apiKey: DEEPSEEK_KEY } },
          idempotencyKey: "user-proxyendpoints-" + Date.now()
        }`
      });
      const result = await runSdkScript(install, env, script, {
        scriptName: "user-proxyendpoints.mjs",
        waitMs: 8 * 60_000,
        timeoutMs: 9 * 60_000
      });

      expect(result.runtime).toBe("managed");
      // The run SUCCEEDS — declaring a proxy endpoint must not break the run;
      // the agent just reports what it found.
      expect(result.status).toBe("succeeded");

      const text = dense(result.assistantText);

      // The bridge file is mounted (validated end-to-end through the SDK).
      // Plane-independent: the runner writes index.json + the runtime bridge into
      // the container regardless of where the proxy is served, so this always
      // holds.
      expect(text).toContain("INDEX_PRESENT");

      // scope: the actual proxy round-trip is served by the dashboard-owned
      // named proxy route at ${ANTPATH_PROXY_PUBLIC_BASE_URL}/api/runs/:id/proxy.
      // Every plane that runs this user test is configured enough to prove the
      // real customer path, not just the mounted bridge files.
      expect(text, `PROXY_OK missing for configured proxy plane: ${text}`).toContain("PROXY_OK");
      expect(text, `PROXY_ERR present for configured proxy plane: ${text}`).not.toContain("PROXY_ERR");
    },
    10 * 60_000
  );
});
