---
title: "Changelog"
description: "Recent public SDK, CLI, and docs changes."
---

# Changelog

This page tracks public developer-facing changes. See the canonical
[`@aexhq/sdk` changelog](https://github.com/aexhq/aex/blob/main/packages/sdk/CHANGELOG.md)
for the complete version history.

## 0.42.0

- Reorganized the SDK around `aex.sessions`, session-owned messages, events,
  files, and webhooks, plus versioned resources under `aex.workspace`.
- Added checkpoint-aware session file access and memory-bounded event iterators.
- Made terminal run events the consistency boundary for session state, usage,
  checkpoints, and files.
- Added automatic immutable canary publication and evidence-backed npm
  promotion.

## Current docs refresh

- Added top-level examples, integrations, support, and changelog pages.
- Added a clearer reference entry point for SDK, CLI, events, and provider/runtime capabilities.
- Tightened mobile code-panel layout so long commands scroll within the page instead of widening the viewport.

## Recently documented surfaces

- Durable sessions, file capture, events, runtime sizes, and provider/model capabilities.
- Workspace secrets, credentials, webhooks, retries, limits, and billing guides.
- CLI parity for start, status, wait, events, tail, inspect, files, download, cancel, delete, auth, models, providers, tools, and runtime sizes.
