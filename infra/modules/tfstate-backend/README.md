# `tfstate-backend`

The S3 bucket and KMS key that hold Terraform state for one plane.

There is no DynamoDB lock table. Terraform's S3 backend takes its lock from an
object in the same bucket when `use_lockfile = true`, which removes a second
resource that could drift out of step with the bucket it guards.

A root consumes this backend with:

```hcl
terraform {
  backend "s3" {
    bucket       = "aex-tfstate-<plane>-<suffix>"
    key          = "<root>/terraform.tfstate"
    region       = "<region>"
    encrypt      = true
    kms_key_id   = "alias/aex-tfstate-<plane>"
    use_lockfile = true
  }
}
```

The backend block cannot take variables, so the values above are written out by
the private root that owns the environment. This module's outputs are what that
root copies from.

## Inputs

| Name | Type | Description |
| --- | --- | --- |
| `plane` | `string` | `dev` or `prd`. |
| `bucket_name_suffix` | `string` | Suffix that makes the name globally unique. |
| `partition` | `string` | AWS partition used to build ARNs. |
| `kms_alias` | `string` | Alias of the state key, `alias/aex-<name>`. |
| `deletion_window_days` | `number` | Key pending-deletion window; floor of 30. |
| `noncurrent_retention_days` | `number` | Days a superseded state version is kept; floor of 30. |
| `tags` | `map(string)` | Tags. |

## Outputs

| Name | Description |
| --- | --- |
| `bucket` | Physical bucket name. |
| `kms_key_arn` | ARN of the state key. |
| `kms_alias_arn` | ARN of the state key alias. |

## Policy asserted

- Versioning is on, and superseded state versions are kept at least 30 days.
- All four public-access-block settings are on.
- The bucket policy denies any request that is not over TLS.
- State is encrypted with a rotating customer-managed key whose deletion window
  is at least 30 days.
- `use_lockfile` is documented above as the locking mechanism; there is no lock
  table resource in this module.

## Not asserted here

Nothing here is reachable from a product deployable, so no live suite owns it.
