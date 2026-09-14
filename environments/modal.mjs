import { createModalEnvironment, serveEnvironment } from "@aexhq/env-modal/server";
import { setTimeout } from "node:timers/promises";

const token = process.env.AEX_ENVIRONMENT_TOKEN;
if (!token || token.length < 32) throw new Error("AEX_ENVIRONMENT_TOKEN requires at least 32 characters");
const origin = new URL(process.env.AEX_OPERATOR_URL ?? "http://127.0.0.1:8082");
if (origin.protocol !== "http:" || !["127.0.0.1", "[::1]"].includes(origin.hostname)
  || origin.username || origin.password || origin.search || origin.hash || origin.pathname !== "/") {
  throw new Error("AEX_OPERATOR_URL must be a literal loopback HTTP origin");
}
const directory = process.env.AEX_ENVIRONMENT_DATA_DIR;
if (!directory) throw new Error("AEX_ENVIRONMENT_DATA_DIR is required on retained disk");
async function control(path, body) {
  const response = await fetch(new URL(`/environments/${path}`, origin), {
    method: body === undefined ? "GET" : "POST", redirect: "error", signal: AbortSignal.timeout(10_000),
    headers: { authorization: `Bearer ${token}`, "content-type": "application/json" },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
  });
  if (!response.ok) throw new Error(`Environment ${path} rejected: HTTP ${response.status}`);
  return response.json();
}
const config = await control("config");
const environment = await createModalEnvironment({ directory, ...config,
  authorize: async binding => (await control("authorize", binding)).expiresAt,
  report: usage => control("usage", usage),
});
const [command, sessionId, name, sandboxId, ...extra] = process.argv.slice(2);
if (command === "recover" && sessionId && name && sandboxId && extra.length === 0) {
  await environment.recover({ sessionId, environment: name, sandboxId });
  environment.close();
  process.exit(0);
}
if (command !== undefined) throw new Error("usage: modal.mjs [recover <session> <environment> <sandbox-id>]");
const port = Number(process.env.AEX_ENVIRONMENT_PORT ?? 8083);
if (!Number.isInteger(port) || port < 1 || port > 65535) throw new Error("invalid AEX_ENVIRONMENT_PORT");
const host = process.env.AEX_ENVIRONMENT_HOST ?? "127.0.0.1";
const server = await serveEnvironment(environment.handle, { token, host, port, maxBodyBytes: 8 * 1024 * 1024 });
const stop = new AbortController();
const reconcile = (async () => {
  while (!stop.signal.aborted) {
    for (const observation of await environment.reconcile()) {
      if (observation.error) console.error(JSON.stringify({ event: "environment_unresolved", ...observation }));
    }
    try { await setTimeout(10_000, undefined, { signal: stop.signal }); }
    catch (error) { if (!stop.signal.aborted) throw error; }
  }
})();
reconcile.catch(error => { console.error(error); process.exit(1); });
for (const signal of ["SIGINT", "SIGTERM"]) process.once(signal, () => {
  stop.abort();
  Promise.all([server.close(), reconcile]).then(() => {
    environment.close();
    process.exit(0);
  }).catch(error => { console.error(error); process.exit(1); });
});
console.log(JSON.stringify({ event: "environment_ready", url: server.url }));
