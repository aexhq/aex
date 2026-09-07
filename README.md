# Aex

Aex hosts [Brain](https://github.com/aexhq/brain) for multiple developer accounts. One Rust
service owns account keys, session and host ownership, hosted admission and operator actions.
The unchanged Brain server owns execution, credentials and durable history.

The MVP provides self-service accounts and API keys, customer model keys, hosted Wasm
Agentloops and Tools, and application Tools running in the customer's process. One serving node accepts explicit
maintenance interruptions. Public hosting implementation lives here; our AWS deployment and
commercial settings live in private Platform. This checkout builds without Platform.

## Development

Install Rust 1.97.1, Node 22+, Python 3 and Git. Linux is required for real Brain workers;
Rust product-boundary tests also run on Windows. Start PostgreSQL 17 and set
`AEX_TEST_DATABASE_URL` to an expendable test database. Tests use isolated schemas.

```sh
cargo build --locked
bash tools/check.sh
bash tools/journeys.sh
docker build -t aex-rewrite:test .
python3 tools/image-smoke.py
```

The journey script fetches the immutable Brain revision in the Cargo manifest, builds its
server, workers and reference Agentloop, then runs the published SDK against Aex. It uses a
local deterministic provider and requires no cloud credentials. All required CI jobs must
pass. Real-provider and deployed recovery evidence additionally gate release.

## Run

Copy `examples/config.json` to your workspace configuration. The `agentloops` list contains
optional operator-approved content addresses; an empty list permits account-owned uploads. Select exact provider/model entries, addresses, PostgreSQL and persistent
Brain storage and measured limits. Generated schemas live under `docs/generated`.

Run the pinned Brain server privately with its own persistent data directory and random
bearer. Set `AEX_BRAIN_TOKEN` to that bearer and `AEX_OPERATOR_TOKEN` to a separate random
operator credential. Set a third distinct `AEX_SITE_TOKEN` for the trusted website and
`AEX_DATABASE_URL` for PostgreSQL (TLS verification in production). Internal credentials
must be at least 32 characters. Local environment files stay at the workspace root.

```sh
aex-server serve --config /path/to/config.json
python3 tools/storage-report.py --brain-data /path/to/brain-data
aex-server operate --request '{"action":"create_account"}'
aex-server operate --request '{"action":"issue_key","account":"ACCOUNT_FROM_PREVIOUS_COMMAND"}'
aex-server operate --request '{"action":"resume"}'
```

**Every start is drained:** customer routes return 503 until operator resume. Resume requires
Brain readiness and fresh metering. The operator listener is loopback-only and uses a separate
credential. Issued customer keys are shown once; only verifiers are stored.

Use `@aexhq/sdk` with the issued API key. `Aex` extends Brain and re-exports the Brain SDK. See [the example](examples/session.mjs),
[supported API](docs/api.md), [operations](docs/operations.md), [roadmap](ROADMAP.md) and [ADRs](docs/adr/README.md).

The production deployment and release evidence live in private Platform. One production
environment uses Vercel, EC2/EBS and Single-AZ RDS PostgreSQL in us-east-1. PostgreSQL stores
product metadata; Brain retains journals, credentials and Components on EBS.
