# `ecr-repository`

One ECR repository with immutable tags and a lifecycle policy that can only
reach unreferenced digests.

Images are always consumed as `<url>@sha256:<digest>`. The tag exists for humans;
the digest is the identity. Tag immutability is what keeps those two from
drifting apart.

## Inputs

| Name | Type | Description |
| --- | --- | --- |
| `name` | `string` | Repository name under the `aex/` namespace. |
| `immutable_tags` | `bool` | Must stay `true`. |
| `scan_on_push` | `bool` | Scan an image when it is pushed. |
| `lifecycle_by_reference` | `object` | `{ untagged_expire_days }`. |
| `kms_key_arn` | `string` | Optional customer-managed key; AES256 when null. |
| `force_delete` | `bool` | Must stay `false`. |
| `tags` | `map(string)` | Tags. |

## Outputs

| Name | Description |
| --- | --- |
| `repository_url` | Repository URL, used only with a digest. |
| `arn` | Repository ARN. |

## Policy asserted

- `image_tag_mutability` is `IMMUTABLE`; `immutable_tags = false` is rejected.
- There is exactly one lifecycle rule, it selects `tagStatus = "untagged"`, and
  its action is `expire`. Nothing in the policy can reach a tagged digest that a
  released manifest may still name.
- The repository cannot be force-deleted while it holds images.
- The repository name must live under the `aex/` namespace.

## Not asserted here

Registry behaviour under push and pull is exercised by the publication lane's
package-integrity receipts, not by a deployed live suite.
