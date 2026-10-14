# 010 — Registered Agentloops

Status: Accepted, 2026-10-14. Owner requested startup registration and inclusion of hosted Aex.

Aex offers the Pi, Codex and OpenCode runtimes registered by its private Brain server at startup.
Brain consumes their extension implementations. Each runtime has its own worker pool and
concurrency limit; sessions select an offered name and pass runtime-specific settings.
Customers cannot choose executable paths, upload Agentloops or alter startup registrations.
An empty Aex `agentloops` allowlist denies all session creation.

This replaces the Agentloop content-address admission in ADR 004. Tool and program admission,
account ownership, quotas and Environment grants keep their existing boundaries. Aex's SDK
prepares hosted Tools, while Agentloop factories only produce selections. Typed response
validation and correction remain Aex SDK policy under ADR 008.

Brain 0.41 changes both the session contract and its on-disk format to `brain-data/3`.
Release requires paired backups and an explicit plan for retained older sessions; no automatic
conversion or deletion is implied. Storage accounting includes journals and native runtime
files under `agentloops/<name>/<session-id>`. Linux journeys exercise all three runtimes,
concurrent admission, browser grants, cancellation, restart and paired restore.
