export const EDGE_CHAT_SESSION_SHARDS = {
  cancelLaunch: {
    file: "test/live/edge-chat-cancel-launch.user.test.ts",
    scenario: "cancel-launch"
  },
  cancelSend: {
    file: "test/live/edge-chat-cancel-send.user.test.ts",
    scenario: "cancel-send"
  },
  concurrency: {
    file: "test/live/edge-chat-concurrency.user.test.ts",
    scenario: "concurrency"
  },
  delete: {
    file: "test/live/edge-chat-delete.user.test.ts",
    scenario: "delete"
  },
  multiturn: {
    file: "test/live/edge-chat-multiturn.user.test.ts",
    scenario: "multiturn"
  },
  replay: {
    file: "test/live/edge-chat-replay.user.test.ts",
    scenario: "replay"
  },
  suspend: {
    file: "test/live/edge-chat-suspend.user.test.ts",
    scenario: "suspend"
  }
} as const;

export type EdgeChatSessionShard =
  (typeof EDGE_CHAT_SESSION_SHARDS)[keyof typeof EDGE_CHAT_SESSION_SHARDS];

export type EdgeChatSessionScenario = EdgeChatSessionShard["scenario"];
