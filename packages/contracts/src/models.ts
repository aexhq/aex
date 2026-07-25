import { rethrowContractParseError } from "./contract-parse-error.js";
import { MODEL_SLUG_PATTERN, modelSlugSchema } from "./schemas/models.js";
import { parseWire } from "./schemas/wire.js";

/**
 * Public model ids are plain **Vercel AI Gateway `creator/model` slug strings**.
 *
 * There is no closed model set and no provider concept: a customer names any
 * slug the managed gateway catalog serves (e.g. `anthropic/claude-haiku-4-5`,
 * `deepseek/deepseek-v4-flash`) and the platform routes it through the single
 * managed gateway key. Adding a catalog model is a ZERO-code change — the slug
 * just works. The boundary validation is purely structural (see
 * {@link parseModelSlug}); an unknown-but-well-formed slug is allowed through so
 * a slightly-old client can still run a newly-launched model (the gateway
 * arbitrates at submit time).
 */
export type ModelName = string;

/**
 * Structural gateway model-slug shape: a lowercase `creator`, a `/`, then the
 * model segment. Matches Vercel AI Gateway `creator/model` slugs
 * (e.g. `anthropic/claude-sonnet-4-6`, `openai/gpt-4.1`, `x-ai/grok-2`).
 */
export { MODEL_SLUG_PATTERN };

/** True when `input` is a structurally valid `creator/model` gateway slug. */
export function isModelSlug(input: unknown): input is string {
  return typeof input === "string" && MODEL_SLUG_PATTERN.test(input);
}

/**
 * Validate a public model id as a gateway `creator/model` slug string. This is
 * a boundary shape gate ONLY — it does not consult a catalog, so a well-formed
 * slug the running client does not recognize is accepted (the gateway rejects a
 * truly-absent model at submit time). Throws a {@link import("./contract-parse-error.js").ContractParseError}
 * with `field` context on a malformed value.
 */
export function parseModelSlug(input: unknown, field = "submission.model"): string {
  try {
    return parseWire(modelSlugSchema(field), input);
  } catch (error) {
    rethrowContractParseError(error, "parseModelSlug");
  }
}
