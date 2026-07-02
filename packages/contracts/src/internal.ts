// Workspace-internal entry point. Re-exports the submission building blocks
// (leaf parsers, helper validators, shared constants) so the platform-only
// `@aexhq/shared` package can reuse them instead of hand-mirroring ~1.8k lines
// of validator code. NOT part of the public `@aexhq/contracts` surface — do
// NOT add this to the package `index`; consumers reach it via the explicit
// `@aexhq/contracts/internal` subpath.
export * from "./models.js";
export * from "./post-hook.js";
export * from "./proxy-protocol.js";
export * from "./proxy-validation.js";
export * from "./submission.js";
