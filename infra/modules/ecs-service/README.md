# `ecs-service`

A long-running Fargate service: `brain-mux` or `regional-stream`.

The image is always a digest. A service that follows a tag can restart onto
different bytes with no deployment, no manifest and no receipt, which would make
every other identity guarantee in the release path decorative.

`brain-mux` is pinned to one task. It holds session state in memory, and a
second task would take turns it has no state for. The pin is enforced three ways
- `desired_count`, the autoscaling ceiling and the validation that rejects both
- and it comes off when the multi-task session-affinity decision lands, not
before.

`desired_count` is in `ignore_changes` so a scaling event does not show up as
drift on the next plan.

## Inputs

| Name | Type | Description |
| --- | --- | --- |
| `name` | `string` | Service name; also selects the `brain-mux` and `regional-stream` pins. |
| `cluster_arn` / `cluster_name` | `string` | Cluster the service runs in. |
| `image` | `string` | Digest-pinned image. |
| `cpu` / `memory` | `number` | Fargate task size. |
| `runtime_platform` | `object` | Explicit architecture and OS family. |
| `desired_count` | `number` | Task count; defaults to 1. |
| `stop_timeout` | `number` | Drain window: 120 for `brain-mux`, 30 for `regional-stream`. |
| `deregistration_delay` | `number` | Target-group drain window; at least 30. |
| `circuit_breaker` | `object` | `{ enable, rollback }`; `enable` must be true. |
| `autoscaling_metrics` | `list(object)` | Target-tracking metrics. |
| `autoscaling_bounds` | `object` | `{ min_capacity, max_capacity }`. |
| `env` / `secret_env` | `map(string)` | Environment; secrets by ARN reference. |
| `container_port` | `number` | Port the container listens on. |
| `target_group_arn` | `string` | Optional target group to register with. |
| `task_role_arn` / `execution_role_arn` | `string` | Roles. |
| `subnets` / `security_group_ids` | `list(string)` | Network placement. |
| `log_group_name` / `region` | `string` | Logging. |
| `tags` | `map(string)` | Tags. |

## Outputs

| Name | Description |
| --- | --- |
| `service_arn` | ARN of the service. |
| `task_definition_arn` | Revisioned task definition ARN. |
| `deregistration_delay` | Drain window for the target group in front of the service. |

## Policy asserted

- The `image` validation regex rejects any `:tag` reference; only
  `<repository>@sha256:<64 hex>` is accepted.
- The deployment circuit breaker is enabled and rolls back; disabling it is
  rejected.
- `desired_count` defaults to 1, and for `brain-mux` any other value is
  rejected, as is an autoscaling ceiling above 1.
- `stop_timeout` is 120 for `brain-mux` and 30 for `regional-stream`; any other
  value for those two services is rejected.
- At least one autoscaling policy tracks a service-published metric. A
  configuration that scales on `AWS/ECS` CPU alone is rejected.
- `deregistration_delay` is at least 30 seconds.
- Tasks never receive a public address.
- Every environment key is namespaced `AEX_*`, and every secret value is an ARN
  reference.

## Not asserted here

Drain behaviour, deregistration timing and the real task-start interval are the
`aws.ecs.run_task` seam, owned by the `brain-mux` and `regional-stream` live
suites.
