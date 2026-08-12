# `aurora-serverless-v2`

The central finance cluster: Aurora PostgreSQL Serverless v2 with the Data API
on and nothing reachable from the internet.

Development may omit a reader. Production may compose exactly one warm failover
reader. Application reads continue to use the Data API cluster resource rather
than the reader endpoint, so the failover instance does not introduce a second
read-consistency path.

The master password is minted and rotated by RDS. The only credential output is
the ARN of that RDS-managed secret; callers pass it as the Data API `secretArn`.
The module does not accept a second caller-supplied secret identity.

## Inputs

| Name | Type | Description |
| --- | --- | --- |
| `cluster_identifier` | `string` | Cluster identifier, `aex-<...>`. |
| `database_name` | `string` | Initial database name. |
| `master_username` | `string` | Master user name; the password is RDS-managed. |
| `engine_version` | `string` | Pinned `<major>.<minor>`. |
| `min_acu` / `max_acu` | `number` | Serverless v2 capacity bounds. |
| `backup_retention_days` | `number` | At least 7. |
| `deletion_protection` | `bool` | Defaults `true`. `false` is legal only with `deletion_protection_override_reason`. |
| `deletion_protection_override_reason` | `string` | Written justification required to run unprotected. Null by default. |
| `skip_final_snapshot` | `bool` | Defaults `false`, so a delete takes a snapshot. |
| `final_snapshot_identifier` | `string` | Snapshot name taken on delete. Required unless the snapshot is skipped, null when it is. |
| `data_api_enabled` | `bool` | Must stay `true`. |
| `publicly_accessible` | `bool` | Must stay `false`. |
| `reader_count` | `number` | Zero or one warm failover reader. |
| `subnet_ids` | `list(string)` | At least two private subnets. |
| `vpc_security_group_ids` | `list(string)` | Security groups. |
| `region` | `string` | AWS region. |
| `kms_key_arn` | `string` | Customer-managed key for storage and the managed secret. |
| `lambda_invoke_role` | `object({ arn = string })` | Optional role associated with the cluster's `Lambda` feature. Object presence enables the association even when a producer's ARN is unknown during planning; its policy is composed outside this module. |
| `preferred_backup_window` | `string` | Daily backup window in UTC. |
| `tags` | `map(string)` | Tags. |

## Outputs

| Name | Description |
| --- | --- |
| `cluster_arn` | Data API `resourceArn`. |
| `endpoint` | Writer endpoint. |
| `reader_endpoint` | Reader endpoint; resolves to the writer with no reader. |
| `managed_master_user_secret_arn` | Secret RDS mints for the master user and the Data API `secretArn`. |

## Policy asserted

- Backup retention is at least 7 days; a shorter retention is rejected.
- Deletion protection is on by default; disabling it without a written
  `deletion_protection_override_reason` is rejected, and so is a blank reason.
- A delete names either a final snapshot or an explicit skip, never neither.
  RDS rejects a delete that names neither, so a module that could express
  neither could not be destroyed at all — the guard here is that the choice is
  made deliberately, not that the choice is unavailable.
- `skip_final_snapshot` and `final_snapshot_identifier` are Terraform-only
  attributes the provider reads from state when it deletes. Changing either one
  takes effect only after an `apply` writes it to state, which is why a teardown
  applies first and destroys second.
- The Data API is on; disabling it is rejected.
- No instance is publicly accessible; setting it is rejected.
- At most one warm failover reader is allowed; larger counts are rejected.
- Storage is encrypted with the supplied customer-managed key.
- The master password is RDS-managed and never appears in configuration.
- The engine version must be pinned to a minor.
- When a Lambda invocation role is supplied, it is associated through the
  cluster's exact `Lambda` feature rather than attached as an application role.

## Not asserted here

ACU scaling, cold resume, failover, Data API statement and result limits, and
commit ambiguity are the `aws.rds_data.transaction` seam. Network reachability
to the regional Lambda API and the role's exact invoke policy belong to the
composing plane.
