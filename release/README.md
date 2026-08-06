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

Release routing is stricter than structural PR/main routing. Running
`graph verify --release` refuses to emit a release selection when every cross-service
scenario is deferred. A dev verification statement likewise requires passing
`smoke`, `e2e`, and `user` receipts for the exact release before it can become
production promotion evidence.

`semantic-receipts.json` is the reviewed producer registry for artifact
semantic evidence. It maps every `contract`, `property`, `conformance`, and
`integration` requirement in `units.toml` to the real package test selection
that earns it. Validation rejects missing, extra, duplicate, or nonexistent
filtered test binaries.

On a protected main push, `_build-artifacts.yml` builds and packages each unit,
preserves its official GitHub provenance bundle, verifies the exact immutable
publication identity, and binds the required build/test/package receipts to
the artifact subject. `artifact certify --defer-supply-chain` records the
startup-phase scanner deferral explicitly; it does not mint a synthetic SBOM,
licence, vulnerability, or deny receipt. Missing build/test receipts,
publication bytes, provenance, signatures, or model-catalog bindings still
stop publication.

Dependency audits, licence inventory, SBOM generation, and vulnerability
scanning are outside the release critical path during startup. Their revisit
trigger and non-blocking operating policy live in
[`references/backlog.md`](../references/backlog.md).
