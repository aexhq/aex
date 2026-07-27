export const LIVE_RUNTIME_KINDS = ["container", "spot_container", "lambda"] as const;

export type LiveRuntimeKind = (typeof LIVE_RUNTIME_KINDS)[number];

/**
 * Every matrixed live file must submit the arm selected by CI. Missing or
 * malformed selection is a hard failure; silently falling back would make a
 * Lambda job rerun the default container and report false parity.
 */
export function requireLiveRuntimeKind(label: string): LiveRuntimeKind {
  const value = process.env["AEX_USER_TEST_RUNTIME_KIND"];
  if (!LIVE_RUNTIME_KINDS.some((kind) => kind === value)) {
    throw new Error(
      `${label}: required env AEX_USER_TEST_RUNTIME_KIND must be one of ${LIVE_RUNTIME_KINDS.join(", ")}, got ${JSON.stringify(value)}`
    );
  }
  return value as LiveRuntimeKind;
}
