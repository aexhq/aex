import { BrainClient, SessionHandle, type CreateSessionRequest, type Session as SessionState, type SessionEvent } from "@aexhq/brain";

export interface RequestOptions {
  idempotencyKey?: string;
}

export class Sessions {
  constructor(private readonly brain: BrainClient) {}

  async create(request: CreateSessionRequest, options: RequestOptions = {}): Promise<Session> {
    return new Session(await this.brain.createSession(request, key(options)));
  }

  async list(): Promise<Session[]> {
    const { sessions } = await this.brain.listSessions();
    return Promise.all(sessions.map(async (state) => new Session(await this.brain.getSession(state.session_id))));
  }

  async get(sessionId: string): Promise<Session> {
    return new Session(await this.brain.getSession(sessionId));
  }
}

export class Session {
  constructor(private readonly handle: SessionHandle) {}

  get state(): SessionState { return this.handle.state; }
  get id(): string { return this.handle.id; }

  async send(content: unknown, options: RequestOptions = {}): Promise<SessionState> {
    return this.handle.send(content, key(options));
  }

  events(after = 0): AsyncIterable<SessionEvent> {
    return this.handle.events(after);
  }

  async cancel(options: RequestOptions = {}): Promise<void> {
    await this.handle.cancel(key(options));
  }

  async end(options: RequestOptions = {}): Promise<SessionState> {
    return this.handle.end(key(options));
  }

  async delete(options: RequestOptions = {}): Promise<void> {
    await this.handle.delete(key(options));
  }
}

function key(options: RequestOptions): string {
  return options.idempotencyKey ?? crypto.randomUUID();
}
