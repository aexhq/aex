# `log-group`

One CloudWatch log group, encrypted with a customer-managed key and always
retained for a bounded window.

It exists for the same reason `ecs-cluster` does: an environment root that has to
declare a raw `resource` block for a resource type with no module cannot satisfy
`no-resource-blocks-except-backend-and-module`, and a log group created outside a
module is a log group with no asserted retention and no asserted key.

Lambda log groups are created by `lambda-function`, which owns the
`/aws/lambda/<function>` name AWS imposes. This module is for the groups the
architecture names itself: ECS services, one-shot tasks and anything else that
writes under `/aex/<plane>/`.

## Inputs

| Name | Type | Description |
| --- | --- | --- |
| `name` | `string` | `/aex/<plane>/<deployable>`. |
| `retention_days` | `number` | A CloudWatch retention window; never unbounded. |
| `kms_key_arn` | `string` | Customer-managed key; the default key is rejected. |
| `tags` | `map(string)` | Tags applied to the group. |

## Outputs

| Name | Description |
| --- | --- |
| `name` | Group name, the value an `awslogs` driver names. |
| `arn` | Group ARN. |

## Policy asserted

- The name is `/aex/<plane>/<deployable>`. Log groups are the one namespace
  expressed as a path rather than a prefix, and the plane has to be inside it or
  a plane-scoped IAM policy cannot reach the group and the deployable writes
  nowhere.
- Retention is one of the windows CloudWatch accepts, and never `0`. A group with
  no retention keeps every line forever and bills for it.
- Encryption is a customer-managed key. Logs carry request identifiers, workspace
  ids and error detail, so there is no unencrypted case.

## Not asserted here

Whether the key policy actually admits `logs.<region>.amazonaws.com` is the
`aws.kms.encryption_context` seam: a log group creates successfully and then
rejects every write when the service principal cannot use the key.
