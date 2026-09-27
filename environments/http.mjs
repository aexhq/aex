import { createHttpEnvironment, serveEnvironment } from "@aexhq/env-http/server";
import { token, control } from "./control.mjs";
import { authenticateApplication } from "./application.mjs";

const environment = createHttpEnvironment({ authorize: async (binding, signal, context) => {
  const admitted = await control("http/authorize", { ...binding, ...(context && { authorization: context.authorization }) }, signal);
  return context ? { ...admitted, token: context.credential } : admitted;
} });
const server = await serveEnvironment(environment.handle, {
  authenticate: headers => authenticateApplication(headers, token), callback: environment.callback,
  port: 8084, maxBodyBytes: 8 * 1024 * 1024,
});
for (const signal of ["SIGINT", "SIGTERM"]) process.once(signal, () => {
  server.close().catch(() => { process.exitCode = 1; });
});
console.log(JSON.stringify({ event: "environment_ready", url: server.url }));
