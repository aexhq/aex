/**
 * Schema for the per-submission security-profile selector.
 *
 * Vocabulary only. What each profile permits — open networking, runtime package
 * installs, customer env vars, MCP servers — is policy evaluated against a
 * submission by `evaluateRuntimeSecurityProfile`, and must not migrate here: a
 * schema that enforced policy would reject a submission for reasons the wire
 * shape cannot express.
 *
 * The token set is declared here, next to the schema built from it, and
 * re-exported by `runtime-security-profile.ts` — see the sibling note in
 * `runtime-kind.ts` for why the constant cannot live on the parser side.
 */
import * as z from "zod/mini";

/** The accepted security-profile values (the wire tokens). */
export const RUNTIME_SECURITY_PROFILES = ["strict", "standard", "developer"] as const;

/**
 * Wire shape of `securityProfile`.
 *
 * Omission (and, for back-compat with records that spelled "unset" as `null`,
 * an explicit null) is handled by `parseRuntimeSecurityProfile`, which resolves
 * it to the `standard` policy rather than to a wire value.
 */
export const RuntimeSecurityProfileSchema = z.enum(RUNTIME_SECURITY_PROFILES, {
  error: (issue) =>
    `securityProfile must be one of: ${RUNTIME_SECURITY_PROFILES.join(", ")} (got ${JSON.stringify(issue.input)})`
});
