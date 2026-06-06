/**
 * USER TEST (SDK-driven) — `proxyEndpoints` round-trip on a managed run.
 *
 * Validates, through the installed SDK, that declaring a `ProxyEndpoint` on a
 * submission both MOUNTS the per-run proxy bridge file
 *   /mnt/session/uploads/aex/index.json
 * AND ships the CONSUMER runtime bridge to the manifest's `aex` path
 * (`/mnt/session/uploads/aex/aex`), so the agent can make a real proxy
 * round-trip. Uses a PUBLIC no-auth upstream (`ProxyEndpoint.none` →
 * httpbin.org) so no secret is involved; `responseMode: "full"` so the JSON
 * body surfaces to the agent.
 *
 * The runner downloads the pinned runtime bridge artifact by digest, verifies
 * sha256, writes /mnt/session/uploads/aex/aex, and the entrypoint drops
 * an `aex` PATH wrapper.
 *
 * Only passes once the Worker is deployed with AEX_RUNTIME_BRIDGE_MANIFEST and
 * AEX_PROXY_PUBLIC_BASE_URL is set (the index.json's
 * `proxyBaseUrl` is composed from it). Runs on managed (deepseek). waitMs ~8min.
 */
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { installAex, type InstallResult } from "../_fixtures/install.js";
import { dense, requireUserEnv, runSdkScript, sdkRunnerScript } from "./_sdk.js";

const env = requireUserEnv({ deepseek: true });

describe("user/SDK: managed proxyEndpoints bridge round-trip succeeds", () => {
  let install: InstallResult;
  beforeAll(async () => {
    install = await installAex();
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
            "Using the shell, run this exact one-liner and reply with its exact final stdout line, no prose:",
            "if test -f /mnt/session/uploads/aex/index.json; then printf 'INDEX_PRESENT '; else printf 'INDEX_MISSING '; fi; if node /mnt/session/uploads/aex/aex proxy httpbin --path /get >/tmp/aex-proxy-response.json 2>/tmp/aex-proxy-error.txt; then printf 'PROXY_OK\\\\n'; else printf 'PROXY_ERR\\\\n'; fi"
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
          secrets: { apiKey: DEEPSEEK_KEY },
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

      const shellEvidence = dense(`${result.toolResultText} ${result.assistantText}`);

      // Assert the shell result, not only the assistant's final prose. The model
      // can summarize only the proxy token even when the shell printed both.
      expect(shellEvidence, `INDEX_PRESENT missing from shell evidence: ${shellEvidence}`).toContain(
        "INDEX_PRESENT"
      );
      expect(
        shellEvidence,
        `INDEX_MISSING reported while proxy should require the manifest: ${shellEvidence}`
      ).not.toContain("INDEX_MISSING");

      // scope: the actual proxy round-trip is served by the API Worker-owned
      // named proxy route at ${AEX_PROXY_PUBLIC_BASE_URL}/api/runs/:id/proxy.
      // Every plane that runs this user test is configured enough to prove the
      // real customer path, not just the mounted bridge files.
      expect(shellEvidence, `PROXY_OK missing for configured proxy plane: ${shellEvidence}`).toContain("PROXY_OK");
      expect(shellEvidence, `PROXY_ERR present for configured proxy plane: ${shellEvidence}`).not.toContain(
        "PROXY_ERR"
      );
    },
    10 * 60_000
  );
});
