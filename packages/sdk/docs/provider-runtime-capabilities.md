---
title: Model access
---

# Model access

Generated from `packages/contracts/src/models.ts`.

Regenerate with `bun run capabilities:generate`; check with `bun run capabilities:check`.

Aex routes every model through the managed Vercel AI Gateway. You name a model
by its gateway `creator/model` **slug** and the platform's single managed key
handles the upstream provider relationship — you never supply a provider API
key, and there is no `provider` field.

## Model ids are gateway slugs

- A model id is a `creator/model` slug string, validated at the boundary by `parseModelSlug` against `^[a-z0-9-]+\/[A-Za-z0-9._:-]+$` (lowercase creator, then `/`, then the model segment).
- Examples: `anthropic/claude-haiku-4-5`, `anthropic/claude-sonnet-4-6`, `deepseek/deepseek-v4-flash`, `openai/gpt-4.1`, `google/gemini-2.5-flash`.
- The catalog is OPEN: adding a model the gateway serves needs zero code — a well-formed slug just works. A slug this SDK does not recognize is still accepted at the boundary and arbitrated by the gateway at submit time; a truly unknown model fails there.

## Streaming

`outputMode: "stream"` is honored for ALL models — every model streams through the gateway. There is no per-model streaming-capability gate.

## Skills

Skills are supplied through the top-level `skills` option. Build one with `Skill.fromDir`, `Skill.fromUrl`, `Skill.fromFiles`, `Skill.fromContent`, or `Skill.fromBytes`; each normalizes to a named workspace skill that the platform snapshots into durable session asset storage.

## Runtime selection is independent of model

Runtime selection is independent of the model: `runtime.kind` accepts `container`, `spot_container`, or `lambda`; `runtime.size` accepts the managed size presets.
