import { createModalEnvironment, serveEnvironment } from "@aexhq/env-modal/server";
import { setTimeout } from "node:timers/promises";

import { token, control } from "./control.mjs";

const directory = process.env.AEX_ENVIRONMENT_DATA_DIR;
if (!directory) throw new Error("AEX_ENVIRONMENT_DATA_DIR is required on retained disk");
const config = await control("config");
for (const { cpu, memoryMiB } of Object.values(config.profiles)) {
  if (cpu !== 1 || memoryMiB !== 1024) throw new Error("sandbox_ms currently prices 1 CPU and 1024 MiB");
}
const environment = await createModalEnvironment({ directory, ...config.configuration, profiles: config.profiles,
  authorize: async binding => (await control("authorize", binding)).expiresAt,
  report: ({ sandboxId, ...usage }) => control("usage", { ...usage, resourceId: sandboxId }),
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
