# `plane-foundation`

Everything one plane needs that is not tied to a single region: the artifact
encryption key, the container registries, the deploy role and the plane-wide
alarms.

It runs after `account-backbone` and before any region root. The split matters
because a registry and a key outlive any one region, and rebuilding them when a
region is added would invalidate every digest already published against them.

The deploy role created here carries no publish action, and the publish role
created by `account-backbone` carries no deploy action. That separation is the
reason the two roles live in different roots.

## Sanitized values

Every value is a variable with no default. No account id, ARN or domain appears
in any `.tf` file here.

## Modules used

| Module | Purpose |
| --- | --- |
| `kms-key` | Plane-wide artifact encryption key. |
| `ecr-repository` | One immutable-tag repository per OCI deployable. |
| `github-oidc-role` | The deploy role. |
| `cloudwatch-alarms` | Plane-wide alarms. |

## Outputs

`artifact_key_arn`, `repository_urls`, `deploy_role_arn`, `alarm_arns`.

## Test

`tests/plane-foundation.tftest.hcl` plans the root against a mock AWS provider
and asserts that every declared repository is created, that each declares an
untagged expiry window, and that the publish and deploy profiles stay disjoint.
