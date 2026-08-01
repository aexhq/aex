# `regional-dynamodb-tables`

Every DynamoDB table in one region, created from the already-decoded contents of
`migrations/regional/generated/regional-tables.json`.

The module **does not read the bundle**. The root decodes it and passes the
resulting object list in as `table_definitions`, together with the digest the
release manifest pins. Terraform packages no code and reads no file, so the
generated bundle stays an artifact with an identity rather than a path.

## Inputs

| Name | Type | Description |
| --- | --- | --- |
| `plane` | `string` | `dev` or `prd`. |
| `region` | `string` | AWS region code. |
| `name_prefix` | `string` | Physical name prefix, `aex-<...>-`. |
| `table_definitions` | `list(object)` | Decoded table definitions. |
| `kms_key_arn_by_authority` | `map(string)` | Customer-managed key per authority. |
| `keystore_physical_name` | `string` | Pinned, immutable physical name of the `keystore` table. |
| `table_definitions_digest` | `string` | Digest of the definitions actually passed in. |
| `expected_definitions_digest` | `string` | Digest the release manifest pins. |
| `tags` | `map(string)` | Tags applied to every table. |

## Outputs

| Name | Description |
| --- | --- |
| `table_names` | Logical name to physical name. This is the map a binding records. |
| `table_arns` | Logical name to table ARN. |
| `stream_arns` | Logical name to stream ARN, for tables with a stream. |

## Policy asserted

- Every table is `PAY_PER_REQUEST`; a provisioned definition is rejected.
- Point-in-time recovery is on with 35 days of retention on every table.
- Deletion protection is on for every table.
- Server-side encryption uses a customer-managed key resolved from the table's
  authority. A table naming an authority with no key is rejected, so there is no
  path to the AWS-managed key.
- No global secondary index uses projection type `ALL`.
- The only permitted TTL attribute name is `expiresAtEpochSeconds`.
- The `keystore` table is created under the pinned physical name and is the one
  table not derived from `name_prefix`; a keystore name derived from the prefix
  is rejected.
- `table_definitions_digest` must equal `expected_definitions_digest`, so a root
  planning against a stale bundle fails before it plans.

## Not asserted here

Stream shard behaviour, TTL actually firing, adaptive capacity and real
contention are the `aws.dynamodb.streams`, `aws.dynamodb.ttl`,
`aws.dynamodb.transact_write` and `aws.dynamodb.condition` seams.

The pinned keystore value itself comes from the environment binding in the
private repository. This module can only prove that the value is used verbatim
and is not derived from the environment.
