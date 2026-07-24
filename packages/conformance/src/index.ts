// @aexhq/conformance — single place to tighten the test bar.
//
// These helpers exist so a Phase-1-style regression (e.g. silencing a
// terminal-event check because the assertion is racy) can't be made by
// inlining a conditional check. Every live/e2e test goes through these
// helpers, so tightening the contract HERE ripples instantly across
// every consumer.
//
// The shape rules (Phase 5 enforces, but the documentation lives here
// because this is where the contract is expressed):
//   1. Every helper must FAIL when its precondition is missing. No
//      "if the field is present, check it" patterns. If a field is
//      legitimately optional, that's a SEPARATE helper (e.g.
//      `expectOptionalField`) that says so in its name.
//   2. Failure messages include a contextual dump (event list, raw
//      result) so a CI failure can be diagnosed without retrying.
//   3. New matchers are added here and consumed via `@aexhq/conformance`,
//      NEVER inlined in the test body. Phase 2's ESLint rule
//      `aex/require-test-contracts-matcher` will eventually enforce
//      this — until then, the convention holds via review + this comment.

export {
  expectTerminalEvent,
  type TerminalOutcome,
  type TerminalEvent
} from "./terminal.js";
export { expectEventStream, type EventStreamShape } from "./event-stream.js";
export { expectStructuredError, type StructuredError } from "./structured-error.js";
// CLI ↔ SDK capability parity: the manifest the `cli-sdk-parity` test asserts
// against the real SDK reflection + CLI verb registry.
export {
  CLI_SDK_PARITY_MANIFEST,
  CLI_PARITY_NOT_SURFACED,
  CLI_PARITY_BARE_LIST,
  CONTROL_PLANE_VERB_BY_CLIENT,
  type CliSdkParityManifest,
  type ControlPlaneSubverbManifest
} from "./cli-sdk-parity.js";
