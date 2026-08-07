# `ecs-service`

A long-running Fargate service: `brain-mux` or `regional-stream`.

The image is always a digest. A service that follows a tag can restart onto
different bytes with no deployment, no manifest and no receipt, which would make
every other identity guarantee in the release path decorative.

`brain-mux` runs at least two tasks in production and exactly one in
development. Session state is not held by the task: the journal, the lease and
fence, and the work and wake state are the authority for every session, so a
second task takes work it can serve. Stable task affinity is an accelerator over
that authority, never a correctness condition, and one task is a single point of
loss for a workload whose state is already durable.

The count is a floor rather than a range: both capacity bounds must equal
`desired_count` for `brain-mux`, so nothing scales it dynamically. Concurrency is
raised on measured alpha evidence, not by a scaling policy.

Per-task concurrency is pinned here too. `brain-mux` must carry
`AEX_MAX_ACTIVE_ACTIVATIONS = "16"`, the approved launch profile, in both planes:
the second production task is a placement decision rather than a larger budget.
That variable is required and defaultless in the binary, so an omitted value is
a container that refuses to start and a different value is a plane running bands
nobody approved. The same number is the Rust-side declaration in
`runtimes/brain-mux/src/admission.rs`; `scripts/validate/brain-mux-launch-profile.test.ts`
holds the two to each other so neither can drift.

`desired_count` is in `ignore_changes` so a scaling event does not show up as
drift on the next plan.

The module has two explicit capacity modes:

- a non-empty `autoscaling_metrics` list creates one Application Auto Scaling
  target plus one target-tracking policy per metric. The existing requirement
  for at least one service-published custom metric still applies;
- an empty metrics list creates no autoscaling target or policy. In that fixed
  mode, `autoscaling_bounds.min_capacity` and `max_capacity` must both equal
  `desired_count`, so the reviewed contract cannot imply scaling that has no
  authoritative metric publisher.

## Inputs

| Name | Type | Description |
| --- | --- | --- |
| `name` | `string` | Service name; also selects the `brain-mux` and `regional-stream` pins. |
| `task_definition_family` | `string` | Plane- and region-qualified family, `aex-<dev\|prd>-<region>-<service-name>`. |
| `cluster_arn` / `cluster_name` | `string` | Cluster the service runs in. |
| `image` | `string` | Digest-pinned image. |
| `cpu` / `memory` | `number` | Fargate task size. |
| `runtime_platform` | `object` | Explicit architecture and OS family. |
| `desired_count` | `number` | Task count; defaults to 1. At least 2 for production `brain-mux`, exactly 1 for development `brain-mux`. |
| `stop_timeout` | `number` | Drain window: 120 for `brain-mux`, 30 for `regional-stream`. |
| `deregistration_delay` | `number` | Target-group drain window; at least 30. |
| `circuit_breaker` | `object` | `{ enable, rollback }`; `enable` must be true. |
| `autoscaling_metrics` | `list(object)` | Target-tracking metrics, or `[]` for fixed-count mode. |
| `autoscaling_bounds` | `object` | `{ min_capacity, max_capacity }`; both equal `desired_count` in fixed-count mode. |
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
- `desired_count` defaults to 1. A production `brain-mux` below two tasks is
  rejected, a development `brain-mux` other than one task is rejected, and for
  `brain-mux` any capacity bound that does not equal `desired_count` is rejected.
- `stop_timeout` is 120 for `brain-mux` and 30 for `regional-stream`; any other
  value for those two services is rejected.
- Non-empty autoscaling configuration must include a policy that tracks a
  service-published metric. A configuration that scales on `AWS/ECS` CPU alone
  is rejected.
- Empty autoscaling configuration creates no target or policy and is accepted
  only with capacity bounds collapsed to `desired_count`.
- `deregistration_delay` is at least 30 seconds.
- Tasks never receive a public address.
- Task-definition revisions are retained on replacement or destroy. Release roles therefore do not need the resource-unscopable ECS deregistration action; plane-aware revision cleanup is a separate operational responsibility.
- Every environment key is namespaced `AEX_*`, and every secret value is an ARN
  reference.
- `brain-mux` carries `AEX_MAX_ACTIVE_ACTIVATIONS = "16"`; an absent or different
  activation budget is rejected in either plane.

## Not asserted here

Drain behaviour, deregistration timing and the real task-start interval are the
`aws.ecs.run_task` seam, owned by the `brain-mux` and `regional-stream` live
suites.
