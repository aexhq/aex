import type { Brain, Event, Input, Session, RuntimeTool } from "@aexhq/brain";
import type { AttachmentMedia } from "./generated.js";
import { structuredOutput, type Schema, type StructuredSendOptions } from "./structured-output.js";

export interface SendOptions { signal?: AbortSignal; idempotencyKey?: string }
export interface Turn { input_id: string; session: Session; events: Event[]; output: string | undefined }
export type UserInput = string | (Omit<Input, "type" | "key" | "media"> & { media?: AttachmentMedia[] });

export class AexSessionHandle {
  private activeSends = 0;
  private structuredSend = false;
  constructor(private readonly client: Brain, public state: Session) {}
  get id(): string { return this.state.id; }

  async submit(input: UserInput, options: SendOptions = {}): Promise<Event> {
    if (this.structuredSend) throw new Error("structured output requires exclusive sends on this session handle");
    return this.admit(input, options);
  }
  private admit(input: UserInput, options: SendOptions): Promise<Event> {
    const normalized = typeof input === "string" ? { text: input } : input;
    if ((typeof normalized?.text !== "string" || !normalized.text.trim()) && !normalized?.media?.length) throw new TypeError("send needs a non-empty text message");
    return this.client.send(this.id, { ...normalized, type: "user.message", key: options.idempotencyKey ?? crypto.randomUUID() } as Input, options.signal);
  }
  send<S extends Schema>(input: UserInput, options: StructuredSendOptions<S>): Promise<S["_output"]>;
  send(input: UserInput, options?: SendOptions): Promise<Turn>;
  async send(input: UserInput, options: SendOptions | StructuredSendOptions<Schema> = {}): Promise<unknown> {
    const output = "output" in options ? options.output : undefined;
    if (this.structuredSend || (output && this.activeSends)) throw new Error("structured output requires exclusive sends on this session handle");
    this.activeSends++;
    this.structuredSend = output !== undefined;
    const execute = async (message: UserInput, operation: SendOptions): Promise<Turn> => {
      const accepted = await this.admit(message, operation);
      return this.outcome(accepted.id, { ...operation, after: accepted.cursor });
    };
    try {
      if (output === undefined) return await execute(input, options);
      return await structuredOutput(input, { ...options, output }, execute);
    } finally { this.activeSends--; this.structuredSend = false; }
  }

  async outcome(input: string, options: SendOptions & { after?: string } = {}): Promise<Turn> {
    const signal = options.signal ?? AbortSignal.timeout(300000);
    const events: Event[] = [];
    let cursor = options.after ?? "";
    for (;;) {
      signal.throwIfAborted();
      const page = await this.client.history(this.id, cursor, 1000, signal);
      cursor = page.next_cursor;
      for (const event of page.events) {
        if (event.input_id !== input) continue;
        events.push(event);
        if (event.type === "session.error") throw new Error("session execution failed", { cause: event.data });
        const status = event.data as { status?: string };
        if (event.type === "session.status" && ["idle", "interrupted", "failed", "closed"].includes(status.status ?? "")) {
          this.state = await this.client.get(this.id, signal);
          if (status.status !== "idle") throw new Error(`session execution ${status.status}`);
          const message = events.findLast(event => event.type === "assistant.message")?.data as { text?: string } | undefined;
          return { input_id: input, session: this.state, events, output: typeof message?.text === "string" ? message.text : undefined };
        }
      }
      const current = await this.client.get(this.id, signal);
      this.state = current;
      if (["interrupted", "failed", "closed"].includes(current.status)) throw new Error("turn outcome unavailable after interruption or recovery");
      if (page.events.length === 1000) continue;
      await new Promise<void>((resolve, reject) => {
        const abort = () => { clearTimeout(timer); reject(signal.reason); };
        const timer = setTimeout(() => { signal.removeEventListener("abort", abort); resolve(); }, 100);
        signal.addEventListener("abort", abort, { once: true });
        if (signal.aborted) abort();
      });
    }
  }
  history(after = "", limit = 100): ReturnType<Brain["history"]> { return this.client.history(this.id, after, limit); }
  events(options: { after?: string; signal?: AbortSignal } = {}): ReturnType<Brain["observe"]> { return this.client.observe(this.id, options); }
  async transcript(): Promise<Event[]> {
    const events: Event[] = [];
    let cursor = "";
    for (;;) {
      const page = await this.history(cursor, 1000);
      events.push(...page.events);
      if (!page.events.length) return events;
      cursor = page.next_cursor;
    }
  }
  interrupt(): Promise<void> { return this.client.interrupt(this.id); }
  async resume(): Promise<Session> { return this.state = await this.client.resume(this.id); }
  close(): Promise<void> { return this.client.close(this.id); }
  rebind(tools: RuntimeTool[]): Promise<void> { return this.client.rebind(this.id, tools, "sse"); }
  delete(signal?: AbortSignal): Promise<void> { return this.client.delete(this.id, signal); }
}
