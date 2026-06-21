/**
 * Single source of truth for the SDK version exposed at runtime.
 *
 * Must match `packages/sdk/package.json`'s `version` field — a unit
 * test asserts this. Bump both on every release.
 *
 * Used by the (future) User-Agent header on outbound SDK requests.
 */
export const SDK_VERSION = "0.26.5";
