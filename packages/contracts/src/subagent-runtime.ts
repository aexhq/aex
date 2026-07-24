/**
 * Narrow internal contract entrypoint for the container-side subagent tool.
 *
 * Keep this leaf free of event-stream and other public barrel imports: the
 * runner image embeds these primitives, while the full internal barrel is
 * allowed to aggregate broader SDK helpers.
 */
export {
  isModelSlug,
  parseModelSlug
} from "./models.js";
export {
  BUILTIN_TOOL_NAMES
} from "./submission.js";
export {
  buildSubagentAssetsInputSchema,
  buildSubagentBuiltinToolsInputSchema,
  parseSubagentAssetsInput,
  parseSubagentBuiltinToolsInput
} from "./subagent-input.js";
export type {
  BuiltinToolName,
  PlatformSubmission,
  ProviderName
} from "./submission.js";
export type { SubagentInputSchema } from "./subagent-input.js";
