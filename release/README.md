# `release/`

Composition manifest, artifact and evidence schemas plus the policy the release
tool enforces. It never contains a live secret.

Production promotes a complete immutable composition manifest; a one-service
release creates a new complete manifest whose only artifact change is that
service's digest.

Each manifest unit also carries the exact applicable `lambda`, `fargate`, or
`microvm` shape copied from its `units.toml` row. The shape is part of the
canonical `releaseId`; changing deployable resources therefore creates a new
composition even when artifact bytes do not change. Shape kind and presence are
validated before publication. MicroVM shapes are plane-neutral image
capabilities, not hosted image identifiers.

The delivery graph distinguishes invalid omissions from explicit prelaunch
architecture debt. `routes-meta.yaml` gives every unmounted operation a
non-empty `deferredOperations` reason, and `scenario-ownership.toml` gives every
scenario without an executable package/target a non-empty `deferred` reason.
Those states are mutually exclusive with real mount/runnable claims, appear in
the graph summary, and are excluded from scenario execution matrices. They are
not receipts and cannot satisfy artifact certification, environment admission,
or promotion readiness; they let structural graph verification prove that no
gap is accidental while the later evidence gates remain fail-closed.

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
