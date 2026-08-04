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
| `table_definitions` | `list(object)` | Decoded table definitions, each optionally carrying `pinned_physical_name`. |
| `kms_key_arn_by_authority` | `map(string)` | Customer-managed key per authority. |
| `table_definitions_digest` | `string` | The `blake3:` digest the decoded bundle carries. |
| `expected_definitions_digest` | `string` | The same digest, as the release manifest pins it. |
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
- No global secondary index uses projection type `ALL`, an `INCLUDE` projection
  names between one and 20 attributes, a `KEYS_ONLY` projection names none,
  and the `INCLUDE` projection count summed across one table is at most 100.
- The only permitted TTL attribute name is `expiresAtEpochSeconds`.
- A table that declares `pinned_physical_name` is created under exactly that
  name; every table that does not is created under `name_prefix`. A pinned name
  derived from `name_prefix` is rejected, and there is no special case on any
  particular logical name — the definition says whether its name is pinned.
- `table_definitions_digest` must equal `expected_definitions_digest`, so a root
  planning against a stale bundle fails before it plans. Both are `blake3:`,
  which is the one digest the bundle carries and the one the release manifest
  pins; Terraform recomputes neither, and a bundle whose content and claimed
  digest disagree is caught by the bundler's own rebuild test.

## Not asserted here

Stream shard behaviour, TTL actually firing, adaptive capacity and real
contention are the `aws.dynamodb.streams`, `aws.dynamodb.ttl`,
`aws.dynamodb.transact_write` and `aws.dynamodb.condition` seams.

A pinned physical name itself comes from the environment binding in the private
repository: it cannot live in the bundle, because a bundle is shared by two
planes in one account and a physical name is not. The bundle records only that
the name *is* pinned; this module can prove that the supplied value is used
verbatim and is not derived from the environment.
