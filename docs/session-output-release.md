# Session output release handoff

The typed-output implementation spans `aex`, `brain`, and hosted platform configuration. Do not
publish the SDK before the matching control plane and Brain revisions are deployed.

## Source order

1. Merge and tag the Brain-owned protocol plus generic external-tool executor support; publish and
   verify `@aexhq/brain` and `@aexhq/brain-tools` before their Aex consumers.
2. Pin that immutable Brain identity in Hands and build the matching `brain-hand-aws` hosted
   composition image.
3. Point Aex at the exact Brain package versions and merge the Aex control-plane, SDK, tools, and
   CLI changes.
4. Configure Brain's `BRAIN_EXTERNAL_TOOL_EXECUTOR_URL` and give Brain's
   `BRAIN_EXTERNAL_TOOL_EXECUTOR_TOKEN` and aex-control's `AEX_EXTERNAL_TOOL_EXECUTOR_TOKEN` the
   same secret. Never put that token in a session, journal, or hand.
5. Deploy the matching hosted Brain/Hand and control-plane revisions, then publish contracts, SDK,
   tools, and CLI.
6. Run the quickstart against production, including one valid output, one bounded repair, one
   final validation failure, cancellation, and one same-key retry.

## npm order

The typed-output baseline was first published as:

1. `@aexhq/contracts@0.26.0`
2. `@aexhq/sdk@0.55.0`
3. `@aexhq/cli@0.26.0`

For this architecture, first publish Brain's SDK and portable Tools packages in dependency order,
then advance Aex versions and preserve this trusted-publishing order: contracts, SDK, tools, CLI.
Verify each exact version is visible before publishing its consumer. The packages request public
access and provenance in their manifests.

## Final proof

Use the example in `docs/quickstart.md` unchanged. Confirm that the stable `aex_submit_output` tool
was present from session creation, ordinary tools remained available, and no provider-native
response-format option or private model call occurred. Verify validation happened in aex-control,
the successful sole tool call completed the turn without another model round, and a retry of the
same Brain call id returned the exact stored executor response.

The per-send schema and any validation feedback are ordinary model-visible messages for that turn.
They must not be placed in the sealed system prefix, the hand/microVM, or provider configuration.
Large files remain assets; inline typed output is bounded by the journal item limit.

Nothing in this handoff deploys services, creates a Git tag, or publishes an npm package by itself.
