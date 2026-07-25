/**
 * Schema for a public model id — a Vercel AI Gateway `creator/model` slug.
 *
 * Structural only. There is no closed model set: an unknown-but-well-formed
 * slug is accepted so a slightly-old client can still name a newly-launched
 * model, and the gateway arbitrates at submit time. See `models.ts`.
 *
 * The pattern is declared here, next to the schema that enforces it, and
 * re-exported by `models.ts` as `MODEL_SLUG_PATTERN` — same reason as the
 * sibling vocabularies: the parser module imports this one, so the constant
 * cannot live there without a cycle.
 */
import * as z from "zod/mini";

/**
 * Structural gateway model-slug shape: a lowercase `creator`, a `/`, then the
 * model segment. Matches Vercel AI Gateway `creator/model` slugs
 * (e.g. `anthropic/claude-sonnet-4-6`, `openai/gpt-4.1`, `x-ai/grok-2`).
 */
export const MODEL_SLUG_PATTERN = /^[a-z0-9-]+\/[A-Za-z0-9._:-]+$/;

/**
 * The field name is part of every message this schema raises, and the same slug
 * rule is mounted at several wire paths (`submission.model`, a session request
 * config's model, an SDK-selected model). Building per path keeps the messages
 * naming the field the caller actually sent; the paths are a small closed set
 * of literals, so the built schemas are memoised rather than rebuilt per call.
 */
const byField = new Map<string, ModelSlugSchema>();

type ModelSlugSchema = ReturnType<typeof buildModelSlug>;

function buildModelSlug(field: string) {
  const notASlugString = `${field} must be a non-empty gateway model slug string ("creator/model")`;
  return z
    .string({ error: notASlugString })
    .check(
      z.minLength(1, { error: notASlugString, abort: true }),
      z.refine((value: string) => MODEL_SLUG_PATTERN.test(value), {
        error: (issue) =>
          `${field} must be a gateway model slug of the form "creator/model" matching ${MODEL_SLUG_PATTERN.source} (got ${JSON.stringify(issue.input)})`,
        abort: true
      })
    );
}

/**
 * The model-slug schema mounted at `field`.
 *
 * Each check aborts so the reported failure is the first one a reader would
 * hit, matching the sequential type/emptiness/pattern ladder it replaces.
 */
export function modelSlugSchema(field: string): ModelSlugSchema {
  const existing = byField.get(field);
  if (existing !== undefined) {
    return existing;
  }
  const built = buildModelSlug(field);
  byField.set(field, built);
  return built;
}
