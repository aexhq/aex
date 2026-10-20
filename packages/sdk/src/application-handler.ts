import type { RuntimeTool, RuntimeContext, ToolScope } from "@aexhq/brain";
import { z } from "zod";

export type ApplicationContext = RuntimeContext & { finish(value: unknown): Promise<void> };
export type ApplicationTool = Omit<RuntimeTool, "run"> & { run(input: unknown, context: ApplicationContext): unknown | Promise<unknown> };

export function createApplicationHandler({ tools, credential, gatewayOrigin = "https://api.aex.dev", fetch: fetcher = fetch }: {
  tools: ApplicationTool[]; credential: string; gatewayOrigin?: string; fetch?: typeof fetch;
}): (request: Request) => Promise<Response> {
  if (credential.length < 32 || credential.length > 4096) throw new TypeError("application credential requires 32 to 4096 characters");
  const gateway = new URL(gatewayOrigin);
  if (gateway.protocol !== "https:" || gateway.origin !== gatewayOrigin) throw new TypeError("gatewayOrigin must be an HTTPS origin");
  const compiled = new Map(tools.map(tool => [tool.name, { tool, input: z.fromJSONSchema(tool.input_schema as z.core.JSONSchema.JSONSchema), output: tool.output_schema ? z.fromJSONSchema(tool.output_schema as z.core.JSONSchema.JSONSchema) : undefined }]));
  const hash = (text: string) => crypto.subtle.digest("SHA-256", new TextEncoder().encode(text)).then(value => new Uint8Array(value));
  const expected = hash(`Bearer ${credential}`);
  return async request => {
    const actual = await hash(request.headers.get("authorization") ?? ""), trusted = await expected;
    let different = 0; for (let index = 0; index < trusted.length; index++) different |= actual[index] ^ trusted[index];
    if (different) return Response.json({ error: "unauthorized" }, { status: 401 });
    if (request.method !== "POST") return Response.json({ error: "method_not_allowed" }, { status: 405 });
    try {
      const reader = request.body?.getReader(); if (!reader) throw new Error("body required");
      const chunks: Uint8Array[] = []; let bytes = 0;
      try { for (;;) { const chunk = await reader.read(); if (chunk.done) break; bytes += chunk.value.byteLength; if (bytes > 131072) throw new Error("body exceeds limit"); chunks.push(chunk.value); } }
      finally { await reader.cancel(); }
      const body = new Uint8Array(bytes); let offset = 0; for (const chunk of chunks) { body.set(chunk, offset); offset += chunk.byteLength; }
      const input = JSON.parse(new TextDecoder().decode(body));
      const selected = compiled.get(input.tool?.name); if (!selected || !Number.isSafeInteger(input.deadline_at_ms) || input.deadline_at_ms <= Date.now() || input.deadline_at_ms > Date.now() + 300000) throw new Error("invalid invocation");
      const callback = new URL(input.completion.url);
      if (callback.origin !== gatewayOrigin || callback.username || callback.password || callback.search || callback.hash || !/^\/v1\/tool-completions\/[a-f0-9]{64}$/.test(callback.pathname)) throw new Error("invalid completion capability");
      const signal = AbortSignal.any([request.signal, AbortSignal.timeout(Math.max(1, input.deadline_at_ms - Date.now()))]);
      const complete = async (body: unknown): Promise<void> => {
        const response = await fetcher(callback, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body), redirect: "error", signal });
        if (!response.ok) throw new Error("application completion was not acknowledged");
      };
      const wrap = async (value: unknown) => { const output = selected.output ? await selected.output.parseAsync(value) : z.json().parse(value); return { content: [{ type: "text", text: JSON.stringify(output) }], structured_content: output }; };
      let finished: unknown;
      const scope: ToolScope = { tenant_id: input.tenant_id, session_id: input.session_id, call_id: input.call_id };
      const output = await selected.tool.run(await selected.input.parseAsync(input.arguments), {
        signal, scope, progress: value => complete({ type: "progress", value }),
        finish: async value => { signal.throwIfAborted(); const result = await wrap(value); await complete({ type: "finish", result }); finished = result; },
      });
      signal.throwIfAborted();
      return Response.json(finished ?? await wrap(output));
    } catch {
      return Response.json({ content: [{ type: "text", text: "application handler failed; effects may have completed" }], is_error: true, unknown_outcome: true });
    }
  };
}
