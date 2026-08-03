# `release/`

Composition manifest, artifact and evidence schemas plus the policy the release
tool enforces. It never contains a live secret.

Production promotes a complete immutable composition manifest; a one-service
release creates a new complete manifest whose only artifact change is that
service's digest.

`semantic-receipts.json` is the reviewed producer registry for artifact
semantic evidence. It maps every `contract`, `property`, `conformance`, and
`integration` requirement in `units.toml` to the real package test selection
that earns it. Validation rejects missing, extra, duplicate, or nonexistent
filtered test binaries.

On a protected main push, `_build-artifacts.yml` packages each unit twice,
preserves its official GitHub provenance bundle, and passes the exact published
bytes or immutable OCI digest through pinned Syft 1.50.0 and Grype 0.116.1.
The public-input job checks CycloneDX 1.6 component and artifact-subject
binding, applies `deny.toml` to the complete license inventory, rejects any
unapproved high or critical vulnerability, turns only completed producer
reports into receipts, and binds them to the draft artifact subject. A unit is
uploaded as `certified-envelopes` only after `artifact certify` has found every
receipt required by its `units.toml` row. Missing tools, empty inventories,
unknown licenses, stale or unidentified advisory data, failed checks, or a
missing provenance/signature bundle stop publication; none becomes a deferral
or a synthetic passing receipt.

`cargo audit` and `bun audit` remain additional exact-lockfile gates. They do
not mint artifact vulnerability receipts: that receipt belongs only to Grype's
scan of the packaged subject and its identified advisory database. Likewise,
the `deny` receipt records cargo-deny's license, ban, and source checks; it does
not relabel the two lockfile audits as work cargo-deny performed.
