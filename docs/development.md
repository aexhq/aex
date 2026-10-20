# Development and self-hosting

Install Rust 1.97.1, Go 1.26, Node.js 24, Python 3, Git and Docker. Start PostgreSQL 17 and set `AEX_TEST_DATABASE_URL` to an expendable database. Tests isolate their schemas. Keep environment files outside this repository.

```sh
npm ci --ignore-scripts
bash tools/check.sh
bash tools/journeys.sh
docker build -t aex-rewrite:test .
python3 tools/image-smoke.py
```

The Go application in `runtime/` consumes an immutable public Brain module. It installs tenant authentication and managed providers; it does not copy the session engine. `tools/journeys.mjs` builds that runtime, installs the pinned native runners and an independently packed SDK, then exercises all three harnesses, Chromium callbacks, usage and paired restore against a recording model provider.

The image packages the Rust gateway, Go application, native runners, reviewed Brain Browser service, and Aex sandbox/application/PDF services. PDF isolation requires Linux root supervision with unprivileged parser UIDs; other application services run as separate unprivileged users.

## Configuration

[examples/config.json](../examples/config.json) declares product policy. `harnesses` and `models` allowlists restrict creation. `managed_providers` defines installed account grants, operations, quotas and profiles. The Go config fixes private listeners, executable commands, provider endpoints, credential-key environment names and managed service addresses. See Brain's [configuration](https://aex.dev/brain/docs).

Use distinct random credentials of at least 32 characters for `AEX_BRAIN_TOKEN`, `AEX_OPERATOR_TOKEN`, `AEX_SITE_TOKEN`, `AEX_ENVIRONMENT_TOKEN`, `AEX_TOOL_TOKEN` and `AEX_MEDIA_TOKEN` where their services are installed. Aex alone receives PostgreSQL, attachment and payment credentials. Modal credentials belong only to the sandbox controller. The stateless application service journals through Aex's private API and receives no database credentials.

Run the runtime on literal loopback, start `aex-server serve --config PATH`, and run `tools/storage-report.py --brain-data PATH`. Every gateway start is drained. Use the operator interface to create an account, issue a key, report storage and resume. Keep the operator listener private.

Generated Rust DTOs come from `contracts/brain.openapi.json`, checked byte-for-byte against the pinned module. Generate product schemas with `cargo run -p aex-server -- contract --output docs/generated`, then run `node tools/sdk-contracts.mjs`. Shared Brain types remain SDK imports.

All affected PR checks and every main CI job gate release. The protected `storage-integration` workflow additionally requires real R2 and Modal credentials and exercises image/PDF/native and remote-browser paths. Missing credentials fail qualification. Immutable release tags bind successful main CI and protected integration results before publication.

[API](api.md) · [Operations](operations.md) · [Architecture](adr/README.md)
