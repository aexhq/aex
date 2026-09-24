import { createHttpEnvironment, serveEnvironment } from "@aexhq/env-http/server";
import { token, control } from "./control.mjs";

const environment = createHttpEnvironment({ authorize: (binding, signal) => control("http/authorize", binding, signal) });
const server = await serveEnvironment(environment.handle, { token, port: 8084, maxBodyBytes: 8 * 1024 * 1024 });
for (const signal of ["SIGINT", "SIGTERM"]) process.once(signal, () => {
  server.close().catch(() => { process.exitCode = 1; });
});
console.log(JSON.stringify({ event: "environment_ready", url: server.url }));
