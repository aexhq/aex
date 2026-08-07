# `aurora-serverless-v2`

The central finance cluster: Aurora PostgreSQL Serverless v2 with the Data API
on and nothing reachable from the internet.

There is no reader instance at launch. A reader would introduce replica lag, and
the finance read paths do not yet distinguish "not there yet" from "not there".
`reader_count` exists as a variable validated to zero so that decision is
explicit in the configuration rather than an omission someone later fills in by
accident.

The master password is minted and rotated by RDS. `admin_secret_arn` is the
secret Data API callers pass as `secretArn`; a precondition checks it lives in
the same region as the cluster, because a cross-region secret fails at runtime
rather than at plan time.

## Inputs

| Name | Type | Description |
| --- | --- | --- |
| `cluster_identifier` | `string` | Cluster identifier, `aex-<...>`. |
| `database_name` | `string` | Initial database name. |
| `master_username` | `string` | Master user name; the password is RDS-managed. |
| `engine_version` | `string` | Pinned `<major>.<minor>`. |
| `min_acu` / `max_acu` | `number` | Serverless v2 capacity bounds. |
| `backup_retention_days` | `number` | At least 7. |
| `deletion_protection` | `bool` | Must stay `true`. |
| `data_api_enabled` | `bool` | Must stay `true`. |
| `publicly_accessible` | `bool` | Must stay `false`. |
| `reader_count` | `number` | Must be `0` at launch. |
| `subnet_ids` | `list(string)` | At least two private subnets. |
| `vpc_security_group_ids` | `list(string)` | Security groups. |
| `admin_secret_arn` | `string` | Secret ARN Data API callers pass. |
| `region` | `string` | Region, checked against the secret ARN. |
| `kms_key_arn` | `string` | Customer-managed key for storage and the managed secret. |
| `preferred_backup_window` | `string` | Daily backup window in UTC. |
| `tags` | `map(string)` | Tags. |

## Outputs

| Name | Description |
| --- | --- |
| `cluster_arn` | Data API `resourceArn`. |
| `endpoint` | Writer endpoint. |
| `reader_endpoint` | Reader endpoint; resolves to the writer with no reader. |
| `admin_secret_arn` | Data API `secretArn`. |
| `managed_master_user_secret_arn` | Secret RDS mints for the master user. |

## Policy asserted

- Backup retention is at least 7 days; a shorter retention is rejected.
- Deletion protection is on; disabling it is rejected.
- The Data API is on; disabling it is rejected.
- No instance is publicly accessible; setting it is rejected.
- There is no reader instance at launch; `reader_count = 1` is rejected.
- Storage is encrypted with the supplied customer-managed key.
- The master password is RDS-managed and never appears in configuration.
- The admin secret must be in the cluster's region; a cross-region ARN fails the
  plan.
- The engine version must be pinned to a minor.

## Not asserted here

ACU scaling, cold resume, failover, Data API statement and result limits, and
commit ambiguity are the `aws.rds_data.transaction` seam.
