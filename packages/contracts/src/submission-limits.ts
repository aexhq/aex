/**
 * Bounds and patterns the submission schemas enforce.
 *
 * A leaf module with no imports. It exists so `schemas/submission-environment.ts`
 * can read these without importing `submission.ts`, which imports the schema
 * back — a cycle that would leave the constants `undefined` at the moment the
 * schemas are constructed at module load.
 *
 * `submission.ts` re-exports every public name from here, so the published
 * surface is unchanged.
 */

/**
 * Reserved prefix for aex-set runtime env vars (`AEX_CLI`,
 * `AEX_RUNTIME_JSON`, …). Customer `environment.envVars` keys carrying this
 * prefix are rejected at submission parse time so platform-set values
 * cannot be silently overwritten.
 */
export const AEX_RESERVED_ENV_PREFIX = "AEX_";

/**
 * Maximum number of `environment.envVars` entries accepted per
 * submission. Picked to be generous for real customer config bags
 * (the broll case ships a handful — `BROLL_STORE`, `BROLL_OUTPUTS`,
 * `BROLL_MODE`, …) while still bounding the size of every RUNTIME
 * file we mount into the container.
 */
export const ENV_VARS_MAX_ENTRIES = 64;

/** Maximum byte length of a single `environment.envVars` value. */
export const ENV_VARS_MAX_VALUE_BYTES = 4096;

/** Maximum total byte length of all `environment.envVars` keys+values combined. */
export const ENV_VARS_MAX_TOTAL_BYTES = 65536;

/**
 * POSIX-shell-portable env var key: starts with `A-Z` or `_`, body is
 * `A-Z`, `0-9`, `_`. We deliberately reject lowercase to keep
 * `RUNTIME.env` readable and consistent with platform conventions; if
 * a customer has lowercase keys today, they uppercase them at the
 * call site.
 */
export const ENV_VAR_KEY_PATTERN = /^[A-Z_][A-Z0-9_]*$/;

/**
 * Package-manager ecosystems accepted by the public submission schema.
 * The customer encodes the target manager as a `name` prefix
 * `"<eco>:<pkg>"` (e.g. "pip:pandas", "npm:express", "apt:ffmpeg"); an
 * UNPREFIXED name defaults to `apt`. After parsing, `PlatformPackage.name`
 * is the bare package and `PlatformPackage.ecosystem` is the resolved
 * manager.
 */
export const PLATFORM_PACKAGE_ECOSYSTEMS = ["apt", "npm", "pip"] as const;
export type PlatformPackageEcosystem = (typeof PLATFORM_PACKAGE_ECOSYSTEMS)[number];

/** POSIX-style env var name a `secretEnv` entry binds to (e.g. `SERPER_API_KEY`). */
export const SECRET_ENV_NAME_PATTERN = /^[A-Za-z_][A-Za-z0-9_]{0,127}$/;

/**
 * Namespace reserved for platform-set values inside the secrets channel. A
 * caller-supplied key carrying it is rejected with its own message rather than
 * the generic unknown-field one, so the reason is legible.
 */
export const PLATFORM_INTERNAL_SECRET_PREFIX = "__aex_";
