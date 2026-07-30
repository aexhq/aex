# `@aexhq/cli`

The explicit, resource-oriented command-line interface for the aex v1 API.
It is a thin adapter over `@aexhq/sdk`; it does not provide compatibility
aliases for the pre-v1 checkpoint/runtime CLI.

```sh
bun add --global @aexhq/cli

aex sessions create \
  --request @session.json \
  --idempotency-key idem_01k4z7x6p9v3h2m8c5r1t0abcd \
  --api-key "$AEX_API_KEY"

aex messages send ses_01k4z7x6p9v3h2m8c5r1t0abcd \
  --request '{"content":[{"type":"text","text":"Ship it"}]}' \
  --idempotency-key idem_01k4z7x6p9v3h2m8c5r1t0abce \
  --api-key "$AEX_API_KEY"

aex events query \
  --session ses_01k4z7x6p9v3h2m8c5r1t0abcd \
  --query @query.json \
  --api-key "$AEX_API_KEY"
```

Run `aex help` for the full resource tree. Mutation bodies use
`--request <json|@file|->`; observational filters use
`--query <json|@file|->`. `-` means stdin.

Durable mutations wait by default. Add `--detach` to print the admitted
operation immediately. A timeout or interrupt only detaches the client; it
does not cancel the server operation.

Downloads mint and consume one short-lived grant:

```sh
aex files persisted download <sessionId> <path> --output <file|->
aex files live download <sessionId> <path> --output <file|-> \
  --wake retained --consistency coherent
aex workspace files download <name> --output <file|->
aex telemetry download <exportId> --output <file|->
aex billing statements download <statementId> \
  --organization <organizationId> --output <file|->
```

The CLI never prints grant URLs. File downloads use a same-directory `.part`
file and atomic rename, reject an existing target unless `--force`, and only
resume an existing partial when `--resume` is explicit.
