/**
 * Public exports for the `@aexhq/cli` package. Tests and the SDK bundle
 * import from here.
 */
export { runCli } from "./run.js";
export { AEX_INDEX_PATH } from "./internal.js";
export type { CliIO } from "./internal.js";
// The CLI verb registry — consumed by the conformance CLI↔SDK parity manifest
// test to assert every SDK capability maps to a registered verb/flag.
export {
  CLI_VERBS,
  CLI_VERB_NAMES,
  OUTPUTS_SUBVERBS,
  RUN_FLAGS,
  findVerbSpec
} from "./host/registry.js";
export type { CliVerbSpec } from "./host/registry.js";
