# `ecs-cluster`

One ECS cluster. It exists so an environment root never has to declare a raw
`resource` block: a root that carries one has a resource nobody reviewed as a
module, and `no-resource-blocks-except-backend-and-module` cannot hold while any
resource type a root needs has no module.

There is no capacity provider and no autoscaling group. Every task in this
architecture is Fargate, so a cluster is a name, an observability level and a
tag set.

## Inputs

| Name | Type | Description |
| --- | --- | --- |
| `name` | `string` | Physical cluster name, composed by the root from the plane and the region. |
| `container_insights` | `string` | `enhanced` or `enabled`. |
| `tags` | `map(string)` | Tags applied to the cluster. |

## Outputs

| Name | Description |
| --- | --- |
| `arn` | Cluster ARN, the value a service or one-shot task names. |
| `name` | Physical cluster name, for the ARNs that need it as a string. |

## Policy asserted

- The name is under the `aex-` namespace, so a cluster cannot be created outside
  the naming scheme the plane guard checks.
- Container Insights is on. `disabled` is rejected by validation rather than
  quietly accepted, because a cluster whose tasks report nothing is a cluster
  nobody can operate during an incident.

## Not asserted here

ECS Exec is enabled per service, not per cluster, and no module in this
repository enables it. If one ever does, the cluster needs an
`execute_command_configuration` with its own encrypted log group, and that is a
change to this module rather than a setting a service can turn on alone.
