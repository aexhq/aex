# `ecs-task-oneshot`

A Fargate task definition for a job that runs once: the central schema admin,
the observation export task, the regional secret key admin.

This module contains **no `aws_ecs_service`**. That is the whole point of it
being a separate module from `ecs-service`. A service would restart a migration
runner that had already decided the schema was up to date, and an exit code
would stop meaning anything.

The task definition is all this module creates. The caller runs it with
`RunTask`, passing the revisioned ARN and the network configuration this module
outputs.

## Inputs

| Name | Type | Description |
| --- | --- | --- |
| `family` | `string` | Task definition family, `aex-<...>`. |
| `image` | `string` | Digest-pinned image, `<repository>@sha256:<64 hex>`. |
| `cpu` | `number` | Fargate task CPU units. |
| `memory` | `number` | Task memory in megabytes. |
| `role_arn` | `string` | Task role the container runs as. |
| `execution_role_arn` | `string` | Execution role for image pull and logs. |
| `subnets` | `list(string)` | Private subnets, passed through to `RunTask`. |
| `assign_public_ip` | `bool` | Must stay `false`. |
| `runtime_platform` | `object` | Explicit CPU architecture and OS family. |
| `log_group_name` | `string` | Log group under `/aex/`. |
| `region` | `string` | Region, for the log driver. |
| `env` | `map(string)` | Environment variables; keys must be `AEX_*`. |
| `secret_env` | `map(string)` | Secret environment variables, by ARN reference. |
| `tags` | `map(string)` | Tags. |

## Outputs

| Name | Description |
| --- | --- |
| `task_definition_arn` | Revisioned task definition ARN. |
| `network_configuration` | Subnets and the always-false public address flag. |

## Policy asserted

- The image is digest-pinned; a `:tag` reference is rejected by validation.
- `assign_public_ip` is `false`, both as an input and in the network
  configuration handed to the caller. Setting it `true` is rejected.
- The runtime platform is declared explicitly rather than inferred.
- Every secret environment value is an ARN reference; a plaintext value is
  rejected.
- Every environment key is namespaced `AEX_*`.
- Task-definition revisions are retained on replacement or destroy. Release roles therefore do not need the resource-unscopable ECS deregistration action; plane-aware revision cleanup is a separate operational responsibility.

## Not expressed as a test

"No service resource" is structural: this module has no `aws_ecs_service`, and
`terraform test` can assert what a plan contains, not what it lacks. The
repository-wide scanner over `infra/**` is where that stays enforced.

## Not asserted here

`RunTask` and `StopTask` lifecycle behaviour is the `aws.ecs.run_task` seam.
