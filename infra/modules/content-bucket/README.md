# `content-bucket`

A regional content-addressed object store. Object keys are content-addressed, so
the bucket is deliberately unversioned: a key either exists with exactly the
bytes its digest names, or it does not exist.

One plane has several such stores — content bodies, observation bodies and
exports — and they differ only in what they hold, so `purpose` names the store
rather than the module hardcoding one. A bucket called
`aex-content-<plane>-<region>-observations` would be a content bucket claiming
to hold observations.

Because there is no version history to fall back on, every guardrail is a bucket
policy denial rather than a convention.

## Inputs

| Name | Type | Description |
| --- | --- | --- |
| `plane` | `string` | `dev` or `prd`. |
| `region` | `string` | AWS region code. |
| `purpose` | `string` | What the store holds; the last component of the name. |
| `partition` | `string` | AWS partition used to build ARNs. |
| `kms_key_arn` | `string` | Customer-managed key; every other key is denied. |
| `lifecycle_role_arn` | `string` | The only principal permitted to delete an object. |
| `signature_age_ms` | `number` | Signature-age ceiling in milliseconds, at most 300000. |
| `abort_incomplete_multipart_days` | `number` | Must be 1, i.e. 24 hours. |
| `tags` | `map(string)` | Tags applied to the bucket. |

The bucket name is `aex-<plane>-<region>-<purpose>`, which is the same
plane-qualified prefix every other resource in a plane carries.

## Outputs

| Name | Description |
| --- | --- |
| `bucket` | Physical bucket name. |
| `bucket_arn` | Bucket ARN. |

## Policy asserted

- Versioning is **disabled**.
- All four public-access-block settings are on.
- S3 Bucket Keys are on and default encryption is SSE-KMS with the supplied
  customer-managed key.
- The bucket policy denies:
  - any request not over TLS;
  - a `PutObject` that carries no `s3:if-none-match` precondition, so no write
    can silently overwrite a content-addressed key;
  - a write whose SSE algorithm is not `aws:kms`, and a write encrypted with any
    key but the content key;
  - any request whose `s3:signatureAge` exceeds the configured ceiling;
  - `s3:DeleteObject` and `s3:DeleteObjectVersion` by every principal except the
    lifecycle role, expressed with `NotPrincipal`.
- Incomplete multipart uploads are aborted after 24 hours.

## Not asserted here

Whether S3 actually enforces those denials is the `aws.s3.conditional_put` and
`aws.s3.unversioned_delete` seam pair. MinIO can prove `If-None-Match` at the API
level, but bucket-policy denial and `s3:signatureAge` are live-only.
