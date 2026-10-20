# Aex roadmap

[ADR 011](docs/adr/011-brain-2-integration.md) defines the Brain 2 integration and clean cutover. The implementation now consumes Brain 2.1.0's immutable public Go module and TypeScript SDK. Code and tests define current behavior; deployment evidence belongs to Platform.

## Implemented

- Rust product gateway and PostgreSQL ownership, API keys, browser grants and revocation.
- Native Pi, Codex and OpenCode with private model credentials and encrypted checkpoints.
- Connected application callbacks, explicit tab attachment/rebind, HTTPS Application endpoints and durable finish acknowledgements.
- Account-granted sandbox, remote-browser, session inspection and installed MCP providers.
- Opaque history cursors and input-correlated acceptance/terminal receipts without automatic action replay.
- Physical-attempt token accounting, explicit unknown usage, prepaid limits and retained financial evidence.
- Immutable owned image/PDF uploads, isolated PDF conversion, and deletion that waits for external cleanup.
- Aex SDK 1.0.0 with composed Brain handles, validated typed responses and corrective turns; customer CLI and account/billing APIs.
- Local native, Chromium, restart and paired-restore journeys. Protected release qualification covers real R2, Modal, PDF conversion through all harnesses and remote Chromium.

The website dashboard retains exactly three items: API keys, Docs, and Account / Billing / Usage. Identity-provider login belongs to Site. Aex supplies authorization and product behavior.

## Release evidence still required

- Successful exact-main CI and protected managed-services qualification for the final Aex revision.
- Immutable SDK/CLI and image publication, admitted Platform release tuple and Site contract/docs regeneration.
- Pre-cutover paired backup, reconciliation of old active claims/resources and fail-fast migration against the production inventory.
- Coordinated deployment of the owning runtime and stateless application/browser service, fresh Brain 2 state, live tenant/usage/media/deletion probes and an isolated paired restore.

There is no compatibility API, legacy-state importer or dual-runtime routing. Historical records are retained with their original recovery pair.

## Deferred

Organizations and SSO, multi-region placement, automatic failover, arbitrary customer executables, general provider endpoint selection, automatic action retry and hidden browser reconnection are outside this release. Add them only after a concrete customer need and a separately accepted boundary.
