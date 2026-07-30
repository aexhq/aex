---
title: "Changelog"
description: "Recent public SDK, CLI, and docs changes."
---

# Changelog

This page tracks public developer-facing changes. See the canonical
[`@aexhq/sdk` changelog](https://github.com/aexhq/aex/blob/main/packages/sdk/CHANGELOG.md)
for package-by-package history.

## Prelaunch v1 refresh

- Made session lifecycle actions explicit: create, stop, persist, fork, discard,
  and delete are separate operations.
- Replaced runtime selectors with one compute-size choice and a resolved compute
  description on the session.
- Made registered files, skills, tools, instructions, and MCP servers
  name-addressed. Setting the same name replaces its current value.
- Added persisted and live file APIs. Live reads may explicitly wake retained
  compute; persisted reads never do.
- Added logs, spans, metrics, traces, and the unified telemetry query, stream,
  listen, gap, and export APIs.
- Added effective workspace-limit reads and explicit account/workspace admission
  state.
- Removed customer callback delivery, public recovery images, runtime selection, resource
  history/copy operations, and compatibility aliases before launch.

## Documentation

- Updated the quickstart, examples, integrations, SDK reference, CLI reference,
  registered-resource guide, file guide, telemetry guide, limits, and billing
  pages for the strict v1 surface.
