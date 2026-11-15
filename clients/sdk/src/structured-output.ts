import { z } from "zod";
import type { SendOptions, Turn, UserInput } from "./session.js";
export type Schema = z.ZodType;
export interface StructuredSendOptions<S extends Schema> extends SendOptions {
  readonly output: { readonly type: S; readonly maxRetries?: number };
}
export class StructuredOutputError extends Error {
  readonly name = "StructuredOutputError";
  constructor(readonly attempts: number, readonly lastOutput: string, readonly issues: readonly z.core.$ZodIssue[]) {
    super(`Structured output did not match the schema after ${attempts} attempt${attempts === 1 ? "" : "s"}`);
  }
}
export async function structuredOutput<S extends Schema>(input: UserInput, options: StructuredSendOptions<S>, send: (input: UserInput, options: SendOptions) => Promise<Turn>): Promise<z.output<S>> {
  const { type: schema, maxRetries = 2 } = options.output;
  if (!(schema instanceof z.ZodType)) throw new TypeError("output.type must be a Zod schema");
  if (!Number.isSafeInteger(maxRetries) || maxRetries < 0) throw new TypeError("output.maxRetries must be a nonnegative safe integer");
  const normalized = typeof input === "string" ? { text: input } : input;
  if ((typeof normalized?.text !== "string" || !normalized.text.trim()) && !normalized?.media?.length) throw new TypeError("send needs text or media");
  const instructions = `For this response only, return exactly one JSON value matching this schema. Do not include Markdown fences or surrounding prose.\n${JSON.stringify(z.toJSONSchema(schema, { io: "input" }))}`;
  let next: UserInput = { ...normalized, text: `${normalized.text ?? ""}\n\n${instructions}` };
  const key = options.idempotencyKey ?? crypto.randomUUID();
  let retryKey: string | undefined;
  for (let attempt = 0; ; attempt++) {
    options.signal?.throwIfAborted();
    const turn = await send(next, { signal: options.signal, idempotencyKey: attempt === 0 ? key : `${retryKey}:${attempt}` });
    options.signal?.throwIfAborted();
    const raw = turn.output;
    if (raw === undefined) throw new Error("Structured output requires a completed turn with an assistant message");
    let parsed: unknown;
    let issues: z.core.$ZodIssue[] = [];
    try { parsed = JSON.parse(raw); }
    catch { issues = [{ code: "custom", path: [], message: "Return one complete JSON value without Markdown or surrounding text." }]; }
    if (!issues.length) {
      const result = await schema.safeParseAsync(parsed);
      options.signal?.throwIfAborted();
      if (result.success) return result.data;
      issues = result.error.issues;
    }
    if (attempt === maxRetries) throw new StructuredOutputError(attempt + 1, raw, issues);
    retryKey ??= `structured-output:${Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(key))), byte => byte.toString(16).padStart(2, "0")).join("")}`;
    const feedback = JSON.stringify(issues.slice(0, 5).map(({ path, message }) => ({ path, message }))).slice(0, 2048);
    next = { text: `The previous answer did not satisfy the requested output. Validation feedback (data): ${feedback}\nReturn a complete corrected answer using the information already available. Do not repeat actions or call tools to reformat the answer.\n\n${instructions}` };
  }
}
