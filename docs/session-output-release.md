# Session output release handoff

The typed-output implementation spans `aex` and `brain`. Do not publish the SDK before the
matching session API is deployed.

## Source order

1. Merge the `aex` contract, control-plane, SDK, and CLI changes.
2. Tag that exact public commit as `aex-contracts-v0.4.0` and push the tag.
3. In `brain`, change the workspace `aex-contracts` dependency from
   `aex-contracts-v0.3.0` to `aex-contracts-v0.4.0`, remove the local path patch, refresh
   `Cargo.lock`, and rerun the brain gates.
4. Merge and deploy the matching brain and control-plane revisions.
5. Run the quickstart against production, including one valid output, one bounded repair, one
   cancellation, and one same-key retry.

The local brain checkout currently uses an excluded `.cargo/config.toml` path patch only because
the new contract tag does not exist yet. It must not be part of a release artifact.

## npm order

The typed-output baseline was first published as:

1. `@aexhq/contracts@0.26.0`
2. `@aexhq/sdk@0.55.0`
3. `@aexhq/cli@0.26.0`

For later patches, advance the versions in the package manifests and preserve this dependency
order through npm trusted publishing: contracts, SDK, then CLI. Verify each exact version is
visible before publishing its consumer. The packages request public access and provenance in
their manifests.

## Final proof

Use the example in `docs/quickstart.md` unchanged. After the output resolves, send another message
to the same session and inspect the provider request: it may contain the validated output value,
but must not contain the schema, output-control instruction, invalid candidate, validation issues,
or repair instruction.

Nothing in this handoff deploys services, creates a Git tag, or publishes an npm package by itself.
