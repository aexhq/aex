---
# GENERATED FILE - do not edit.
# Written by `scripts/docs/generate-all.mjs` from `packages/sdk/docs/concepts/agent-tools.md`.
# Edit the source and run `bun run docs:generate`. A hand edit here is
# reverted by the next `bun run lint`, which regenerates via `prelint`.
title: Agent tools
---

# Agent tools

The hosted agent harness exposes its built-in tool catalog in each session's
resolved configuration. Callers configure inputs through registered resources,
network policy, packages, credentials, and approval policy; they do not select
an execution implementation.

Use registered tools for caller-owned bundles and MCP registrations for trusted
remote tool servers. Both are overwrite-by-name workspace resources.
