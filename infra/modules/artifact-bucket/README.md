# `artifact-bucket`

A versioned bucket for small infrastructure artifacts: Lambda ZIPs, module
bundles, plan files and the like.

This is the mirror image of `content-bucket`. Content is content-addressed and
unversioned; infrastructure artifacts are addressed by key plus object version,
so versioning is on and a bad object can be rolled back to its predecessor.

Object Lock stays off. These are build outputs, not records, and a legal hold
would make retention impossible to enforce.

## Inputs

| Name | Type | Description |
| --- | --- | --- |
| `plane` | `string` | `dev` or `prd`. |
| `bucket_name_suffix` | `string` | Suffix that makes the name globally unique. |
| `partition` | `string` | AWS partition used to build ARNs. |
| `retention_days` | `number` | Days a noncurrent version is kept. |
| `kms_key_arn` | `string` | Optional customer-managed key; SSE-S3 when null. |
| `object_lock_enabled` | `bool` | Must stay `false`. |
| `tags` | `map(string)` | Tags applied to the bucket. |

The bucket name is `aex-infra-artifacts-<plane>-<suffix>`.

## Outputs

| Name | Description |
| --- | --- |
| `bucket` | Physical bucket name. |
| `bucket_arn` | Bucket ARN. |

## Policy asserted

- Versioning is **enabled**.
- Object Lock is off, and enabling it is rejected by validation.
- A lifecycle rule expires noncurrent versions after `retention_days`, and
  aborts incomplete multipart uploads after 24 hours.
- All four public-access-block settings are on and the bucket policy denies any
  request that is not over TLS.

## Not asserted here

Nothing about deployed product behaviour. This bucket holds infrastructure
artifacts only; no live suite owns it.
