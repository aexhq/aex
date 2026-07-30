export const LIVE_V1_SCENARIOS = {
  explicitSessionRoundTrip: {
    id: "v1.explicit-session-round-trip",
    plane: "dev",
    file: "test/live/v1-session.user.test.ts",
    proves: [
      "clean-installed SDK session creation",
      "explicit message admission and durable run polling",
      "durable cascade-delete operation"
    ]
  }
} as const;
