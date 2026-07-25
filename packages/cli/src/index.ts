/**
 * Public exports for the `@aexhq/cli` package. Tests and the SDK bundle
 * import from here.
 */
export { executeCli } from "./main.js";
export { AEX_INDEX_PATH } from "./internal.js";
export type { CliIO } from "./internal.js";
// The CLI verb registry — the declarative surface behind per-verb `--help`
// and the dispatch-vs-registry ownership test.
export {
  CLI_VERBS,
  CLI_VERB_NAMES,
  FILES_SUBVERBS,
  START_FLAGS,
  findVerbSpec
} from "./host/registry.js";
export type { CliVerbSpec } from "./host/registry.js";
