/** Required capabilities covered by the fast published-artifact smoke gate. */
export const PUBLISHED_ARTIFACT_SMOKE_FILES = {
  managedSdkRoundTrip: "test/live/live-sdk-deepseek.test.ts",
  installedCliRoundTrip: "test/live/live-cli-installed.test.ts"
} as const;
