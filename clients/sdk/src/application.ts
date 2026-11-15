import { ManagedTool, type Tool } from "@aexhq/brain";

export function Application({ endpoint, credential, tools, alias = "application", timeoutMs = 300000 }: {
  endpoint: string; credential: string; tools: Tool[]; alias?: string; timeoutMs?: number;
}) {
  const url = new URL(endpoint);
  if (url.protocol !== "https:" || url.port && url.port !== "443" || url.username || url.password || url.search || url.hash) throw new TypeError("application endpoint requires HTTPS on port 443");
  if (!Number.isSafeInteger(timeoutMs) || timeoutMs < 1 || timeoutMs > 300000) throw new TypeError("application timeout must be within 300000 ms");
  if (credential.length < 32 || credential.length > 4096 || /[\u0000-\u0020\u007f-\uffff]/u.test(credential)) throw new TypeError("application credential requires 32 to 4096 printable ASCII characters");
  return ManagedTool("application", { alias, operations: tools.map(tool => tool.name), parameters: { endpoint, timeout_ms: timeoutMs, tools }, credential: { token: credential } });
}
