/**
 * Required capabilities covered by the fast published-artifact smoke gate.
 *
 * These are the richer successors to the retired live-sdk-deepseek and
 * live-cli-installed files: the comprehensive SDK composition run and the CLI
 * runtime-spotcheck anchor. Both drive a clean installed artifact.
 */
export const PUBLISHED_ARTIFACT_SMOKE_FILES = {
  managedSdkRoundTrip: "test/live/live-sdk-comprehensive.test.ts",
  installedCliRoundTrip: "test/live/edge-cli.user.test.ts"
} as const;
