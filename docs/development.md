# Development and self-hosting

Install Rust 1.97.1, Node.js 22+, Python 3, Git and Docker.
Linux is required for real Brain workers; Rust product-boundary tests also run on Windows.

Start PostgreSQL 17 and set `AEX_TEST_DATABASE_URL` to an expendable test database.
Tests use isolated schemas. Keep local environment files at the workspace root.

```sh
npm ci --ignore-scripts
cargo build --locked
bash tools/check.sh
bash tools/journeys.sh
docker build -t aex-rewrite:test .
python3 tools/image-smoke.py
```

The journey script fetches the pinned Brain release and builds the reference Agentloop.
It runs the SDK against real Linux workers and a deterministic provider, without cloud credentials.
All CI checks gate releases; deployed provider and recovery checks additionally gate hosting releases.

PR CI runs checks for affected components; prose-only README/docs changes skip builds and runtime tests.
Generated contracts still require verification. CI changes and unclassified files run every job.
Pushes to `main` run the full suite; the release gate requires every selected job to pass.

## Run a server

Copy [examples/config.json](../examples/config.json) to your workspace configuration.
Choose provider/model entries, addresses, PostgreSQL, persistent Brain storage and capacity limits.
The `agentloops` list permits approved content addresses; an empty list permits account-owned uploads.

Run the pinned Brain server privately with persistent storage and a random bearer token.
Set these variables for Aex; service credentials must be distinct and at least 32 characters:

| Variable | Purpose |
| --- | --- |
| `AEX_BRAIN_TOKEN` | Brain's private bearer credential |
| `AEX_OPERATOR_TOKEN` | Operator access |
| `AEX_SITE_TOKEN` | Trusted website identity bridge |
| `AEX_DATABASE_URL` | PostgreSQL connection URL; verify TLS in production |

```sh
aex-server serve --config /path/to/config.json
python3 tools/storage-report.py --brain-data /path/to/brain-data
```

In another terminal, use the operator interface to create a test account and key:

```sh
aex-server operate --request '{"action":"create_account"}'
aex-server operate --request '{"action":"issue_key","account":"ACCOUNT_FROM_PREVIOUS_COMMAND"}'
aex-server operate --request '{"action":"resume"}'
```

Every start is drained: customer routes return 503 until operator resume.
Resume requires Brain readiness and fresh storage metering. Continue running the meter periodically.
The operator listener is loopback-only; issued API keys are shown once and stored as verifiers.

PostgreSQL stores product metadata; Brain retains journals, credentials and Components on disk.
This repository builds independently of the private hosted deployment configuration.

[Supported API](api.md) · [Operations and recovery](operations.md) · [Architecture decisions](adr/README.md)
