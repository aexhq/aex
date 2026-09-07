# Aex

Aex hosts [Brain](https://github.com/aexhq/brain) for multiple developer accounts. One Rust
service owns account keys, session and host ownership, hosted admission and operator actions.
The unchanged Brain server owns execution, credentials and durable history.

The MVP is an invite-only API with customer model keys, curated Agentloop Components and
application Tools running in the customer's process. One serving node accepts explicit
maintenance interruptions. Public hosting implementation lives here; our AWS deployment and
commercial settings live in private Platform. This checkout builds without Platform.

## Development

Install Rust 1.97.1, Node 22+, Python 3 and Git. Linux is required for real Brain workers;
Rust product-boundary tests also run on Windows.

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

Copy `examples/config.json` to your workspace configuration. Replace the Agentloop placeholder
with the reviewed Component SHA-256. Select exact provider/model entries, addresses, persistent
product storage and measured limits. Generated schemas live under `docs/generated`.

Run the pinned Brain server privately with its own persistent data directory and random
bearer. Set `AEX_BRAIN_TOKEN` to that bearer and `AEX_OPERATOR_TOKEN` to a separate random
operator credential, both at least 32 characters. Local environment files stay at the workspace root.

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

Use the Brain SDK with the Aex URL and issued account key. See [the example](examples/session.mjs),
[supported API](docs/api.md), [operations](docs/operations.md), [roadmap](ROADMAP.md) and [ADRs](docs/adr/README.md).

AWS provisioning, deployed TLS/SSE, actual instance capacity, EBS replacement, snapshot
restoration and onboarding remain held. Local success is not a production release.
