/**
 * Wire schemas, published as the supported validation surface.
 *
 * **Depend on `~standard`, not on Zod.** Every schema here implements
 * [Standard Schema](https://standardschema.dev) v1, so a consumer can validate
 * against it without importing — or agreeing with — our schema library:
 *
 * ```ts
 * import { SessionWebhookSchema } from "@aexhq/contracts";
 *
 * const result = await SessionWebhookSchema["~standard"].validate(input);
 * if (result.issues) { /* … *\/ }
 * ```
 *
 * That indirection is what keeps the choice of Zod reversible: if it is ever
 * replaced, the published interface does not move — only what sits behind
 * `~standard` does.
 *
 * These schemas follow the same semver contract as the exported types: widening
 * one is a minor, tightening one is a breaking change.
 *
 * Note for tooling authors: `~standard.jsonSchema` is **absent** here. These are
 * `zod/mini` schemas and the JSON Schema converter is tree-shaken out of that
 * entrypoint. JSON Schema conversion is a build-time concern — the OpenAPI
 * generator imports full `zod` off these same objects, which works because both
 * entrypoints construct the same core classes.
 */
export { SessionWebhookSchema } from "./session-webhook.js";
export { SessionLimitsSchema, normalizeSessionLimits } from "./session-limits.js";
export type { SessionLimitsWire } from "./session-limits.js";
export { SessionMachineSchema, normalizeSessionMachine } from "./session-machine.js";
export type { SessionMachineWire } from "./session-machine.js";
