---
title: Spike — vendored models.dev snapshot and generated admit table
description: Snapshot facts, per-provider coverage, gateway id shapes, the generator, and the hash-pin workflow for the build-time codegen.
keywords:
  - models.dev
  - codegen
  - vendor
  - spike
audience: implementation agents and maintainers
status: accepted
last_verified: 2026-08-13
related:
  - references/model-provider-library-simplification-2026-08-13/design-2026-08-13.md
---

# Spike: vendored models.dev snapshot and generated admit table

Artifacts: `C:\Users\luowe\AppData\Local\Temp\opencode\spike-modelsdev`
(`api.json`, `gen.ts`, `models_table.rs`).

## Snapshot facts

- `https://models.dev/api.json`: 3,658,229 bytes, 184 providers, single JSON
  object, **no schema-version field** — the content hash is the version.
- SHA-256 `6f11fed1…c1`; Cloudflare ETag is exactly the hash prefix.
  No Last-Modified; `must-revalidate` cache policy.
- Provider shape: `id, env, npm, name, doc, models`; `api` is **absent** (not
  null) for openai/anthropic/google/vercel, present for deepseek
  (`https://api.deepseek.com`), zai (`https://api.z.ai/api/paas/v4`),
  moonshotai (`https://api.moonshot.ai/v1`), openrouter
  (`https://openrouter.ai/api/v1`).

## Coverage (verified)

| provider | npm | models | tool=true |
| --- | --- | --- | --- |
| openai | `@ai-sdk/openai` | 47 | 38 |
| anthropic | `@ai-sdk/anthropic` | 13 | 13 |
| google | `@ai-sdk/google` | 38 | 21 |
| deepseek | `@ai-sdk/openai-compatible` | 4 | 4 |
| zai | `@ai-sdk/openai-compatible` | 14 | 14 |
| moonshotai | `@ai-sdk/openai-compatible` | 10 | 10 |
| openrouter | `@openrouter/ai-sdk-provider` | 349 | 282 |
| vercel | `@ai-sdk/gateway` | 326 | 191 |

The proposal's tool-capable counts (openai 38, anthropic 13, google 21,
deepseek 4, zai 14, moonshotai 10) verified exactly. All six known models
(`deepseek-v4-flash`, `deepseek-v4-pro`, `claude-opus-4-5`, `gemini-2.5-flash`,
`glm-4.6`, `kimi-k2.5`) present under current ids, all `tool_call: true`.

## Gateway id shapes (risk)

All 349 openrouter and 326 vercel ids contain `/` (`microsoft/phi-4`,
`prodia/flux-fast-schnell`); 11 openrouter ids carry a `~` prefix. Raw
models.dev ids can never be URL-path segments — routing must look the id up
in the compiled table and pass it as the model body field only. Vercel models
carry an extra `status` field (deprecated).

## Generator and output

- Generator: TypeScript + `bun` (150 LOC). Runs identically on Windows dev and
  Linux CI. (A Rust bin is an alternative if in-`build.rs` generation is ever
  wanted; the checked-in-file convention makes the TS script sufficient.)
- Output: 801 rows, 162,150 bytes, compiles clean on rustc 1.97.1.
  Row: `{ provider, model, context_window_tokens, max_output_tokens,
  tool_call, dialect_class }`; provider meta carries `base_url`
  (+ `api_from_models_dev: bool`), `npm`, and
  `GENERATED_FROM_SHA256: &str` for provenance.
- Missing-field facts across the 801 rows: `limit.output` **never missing**;
  `limit.context` zero only on image/embedding models (all tool_call=false);
  `tool_call` always present; `temperature` absent on 4 vercel models.
  Policy: refuse admission on missing `limit.output`/`tool_call` (fail fast,
  no default); treat `temperature`/`structured_output` as optional.

## Hash-pin workflow

- CI: `sha256sum -c scripts/models.digest` (committed digest file), then
  `bun scripts/gen-models.ts && git diff --exit-code -- <generated module>`
  — the checked-in table must match the pinned snapshot exactly.
- Refresh: `scripts/update-models` (~35 LOC): fetch → hash → unchanged? exit
  → regenerate → open a PR titled with the old→new digest. The SHA-256 *is*
  the version; no timestamp to consult.
