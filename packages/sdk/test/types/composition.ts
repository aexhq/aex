import type { Aex, AexSessionHandle, Brain, SessionHandle, SessionState } from "../../dist/index.js";
import { brainEnv } from "../../dist/index.js";
import { pi } from "@aexhq/agentloop-pi";

declare const aex: Aex;
declare const session: AexSessionHandle;
// Product composition must continue to expose the neutral public operations.
const clientOperations: Pick<Brain, Exclude<keyof Brain, "sessions">> = aex;
const sessionOperations: Pick<SessionHandle, keyof SessionHandle> = session;
const reopened: Promise<AexSessionHandle> = aex.sessions.get("session");
const listed: Promise<SessionState[]> = aex.sessions.list();
const created: Promise<AexSessionHandle> = aex.sessions.create({
  model: { provider: "openai", name: "gpt-4.1-mini", apiKey: "fixture" },
  agentloop: pi({ env: brainEnv({ name: "brain" }) }),
});
// @ts-expect-error Product handles do not inherit Brain's concrete private state.
const raw: SessionHandle = session;
void [clientOperations, sessionOperations, reopened, listed, created, raw];
