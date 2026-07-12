import { registerEdgeChatSessionScenario } from "../_fixtures/edge-chat-session.js";
import { EDGE_CHAT_SESSION_SHARDS } from "../_fixtures/edge-chat-session-manifest.js";

registerEdgeChatSessionScenario(EDGE_CHAT_SESSION_SHARDS.delete, import.meta.url);
