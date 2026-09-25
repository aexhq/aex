import type { SessionHandle, SessionState, Schema, SchemaOutput, SendOptions, UserInput } from "@aexhq/brain";
import { structuredOutput, type StructuredSendOptions } from "./structured-output.js";

export class AexSessionHandle {
  private activeSends = 0;
  private structuredSend = false;

  constructor(private readonly session: SessionHandle) {}

  get id(): string { return this.session.id; }
  get state(): SessionState { return this.session.state; }
  set state(value: SessionState) { this.session.state = value; }
  get environments(): SessionHandle["environments"] { return this.session.environments; }

  async submit(...args: Parameters<SessionHandle["submit"]>): Promise<number> {
    if (args[1] !== undefined && "output" in args[1]) throw new TypeError("submit requires structured output to run in the hosted Agentloop");
    if (this.structuredSend) throw new Error("structured output requires exclusive sends on this session handle");
    return this.session.submit(...args);
  }

  send<S extends Schema>(input: UserInput | string, operation: StructuredSendOptions<S>): Promise<SchemaOutput<S>>;
  send(input: UserInput | string, operation?: SendOptions): Promise<SessionState>;
  async send(input: UserInput | string, operation: SendOptions | StructuredSendOptions<Schema> = {}): Promise<unknown> {
    const normalized = typeof input === "string" ? { message: input } : input;
    if (typeof normalized?.message !== "string" || normalized.message === "") throw new TypeError("send needs a non-empty message");
    const output = "output" in operation ? operation.output : undefined;
    if (this.structuredSend || (output !== undefined && this.activeSends !== 0)) throw new Error("structured output requires exclusive sends on this session handle");
    this.activeSends++;
    this.structuredSend = output !== undefined;
    const options = { signal: operation.signal, idempotencyKey: operation.idempotencyKey };
    try {
      if (output === undefined) return await this.session.send(normalized, options);
      return await structuredOutput(normalized, { ...options, output }, options.idempotencyKey ?? crypto.randomUUID(),
        (message, sendOptions) => this.session.send(message, sendOptions),
        (after) => this.events(after, options.signal), () => this.state.lastSequence);
    } finally {
      this.activeSends--;
      this.structuredSend = false;
    }
  }

  transcript(): ReturnType<SessionHandle["transcript"]> { return this.session.transcript(); }
  outcome(...args: Parameters<SessionHandle["outcome"]>): ReturnType<SessionHandle["outcome"]> { return this.session.outcome(...args); }
  events(...args: Parameters<SessionHandle["events"]>): ReturnType<SessionHandle["events"]> { return this.session.events(...args); }
  stream(...args: Parameters<SessionHandle["stream"]>): ReturnType<SessionHandle["stream"]> { return this.session.stream(...args); }
  interrupt(...args: Parameters<SessionHandle["interrupt"]>): ReturnType<SessionHandle["interrupt"]> { return this.session.interrupt(...args); }
  end(...args: Parameters<SessionHandle["end"]>): ReturnType<SessionHandle["end"]> { return this.session.end(...args); }
  delete(...args: Parameters<SessionHandle["delete"]>): ReturnType<SessionHandle["delete"]> { return this.session.delete(...args); }
}
