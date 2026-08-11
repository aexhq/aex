# `lambda-function`

One Lambda function, its log group and its alias.

**Terraform packages no code.** There is no `filename` input, no
`archive_file`, no `source_code_hash` over a local path and no build step. The
ZIP was compiled, packaged, digested and published by the release lane; this
module only names the bucket, key and object version, and refuses to deploy if
the bytes S3 is holding are not the bytes the release manifest pins.

## Inputs

| Name | Type | Description |
| --- | --- | --- |
| `function_name` | `string` | Physical function name, `aex-<...>`. |
| `artifact_bucket` | `string` | Bucket holding the published ZIP. |
| `artifact_key` | `string` | `lambda/<unit>/<sha256-hex>.zip`. |
| `artifact_object_version` | `string` | S3 object version of the ZIP. |
| `artifact_sha256` | `string` | S3 `ChecksumSHA256`, base64 of the raw digest. |
| `architecture` | `string` | `arm64` or `x86_64`. |
| `runtime` | `string` | `provided.al2023` or `nodejs22.x`. |
| `handler` | `string` | Handler symbol; `bootstrap` for the custom runtime. |
| `memory_mb` | `number` | 128-10240. |
| `timeout_s` | `number` | 1-900. |
| `reserved_concurrency` | `number` | `-1` for unreserved. |
| `env` | `map(string)` | Environment variables; keys must be `AEX_*`. |
| `role_arn` | `string` | Execution role from `iam-deployable-role`. |
| `log_retention_days` | `number` | CloudWatch retention. |
| `log_kms_key_arn` | `string` | Optional log-group key. |
| `code_signing_config_arn` | `string` | Optional code-signing configuration. |
| `alias_name` | `string` | Alias every caller targets; defaults to `live`. |
| `public_function_url_enabled` | `bool` | Opt-in alias-qualified unauthenticated buffered Function URL; defaults to `false`. |
| `async_failure_destination_arn` | `string` | Optional unconsumed SQS failure destination; setting it enables the alias-qualified async policy. |
| `async_max_event_age_seconds` | `number` | Async event age, 60-21600; defaults to 21600. |
| `async_max_retry_attempts` | `number` | Function-error retries, 0-2; defaults to 2. |
| `tags` | `map(string)` | Tags. |

The `artifact_sha256` value is the same digest the artifact envelope records,
expressed the way S3 returns it. The envelope carries `sha256:<hex>`; S3
`ChecksumSHA256` is base64 of the same 32 raw bytes.

## Outputs

| Name | Description |
| --- | --- |
| `function_arn` | Unqualified function ARN. |
| `version` | Published version minted by this deployment. |
| `alias_arn` | Alias ARN; this is what event sources and callers target. |
| `public_function_url` | Alias-qualified HTTPS URL, or `null` when disabled. |
| `log_group_name` | Log group name. |

## Policy asserted

- There is no `filename` input and the resource never sets one. Code comes from
  `s3_bucket` / `s3_key` / `s3_object_version` only.
- A `data "aws_s3_object"` postcondition and a function precondition both assert
  that the stored object's `checksum_sha256` equals `artifact_sha256`. A
  mismatch fails the plan, so unidentified bytes are never deployed.
- `publish = true`, so the alias always points at an immutable version.
- Every environment key matches `^AEX_[A-Z0-9_]+$`, and a value that looks like
  a credential is rejected: secrets are referenced through the binding.
- The artifact key must be digest-addressed; a mutable key such as `latest.zip`
  is rejected.
- The alias is never `$LATEST`.
- Public HTTPS ingress is opt-in, targets the alias, uses the buffered Function
  URL request contract and adds no browser CORS policy. AWS provider 6.x creates
  the two required alias resource-policy statements: `InvokeFunctionUrl` is
  conditioned on auth type `NONE`, while `InvokeFunction` is restricted by
  `InvokedViaFunctionUrl`. The live release readback verifies both statements.
- When an asynchronous failure destination is supplied, its retry/age policy
  qualifies the immutable alias and sends exhausted events to that SQS queue.

## Not asserted here

Whether the deployed code behaves is the owning deployable's live suite. This
module proves only the identity of the bytes and the shape of the deployment.
