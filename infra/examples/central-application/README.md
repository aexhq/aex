# `central-application`

The central plane: the finance cluster, the finance API, the settlement queue
and its consumer, the one-shot schema admin task, and the schedules that drive
them.

Two things in here are worth copying rather than reinventing.

The Lambda artifact is named by bucket, key and object version, and the module
refuses to plan if the checksum S3 reports does not match the digest the release
manifest pins. Terraform builds nothing.

The schedule targets are resolved from module outputs through a `local`, never
written down as names. A schedule in this root can therefore only ever point at
an alias or a revisioned task definition this root actually created; there is no
spelling of a mutable target that would still plan.

The schema admin input includes an explicit `stop_timeout`. The example passes
that release-pinned value directly into the one-shot task definition so ECS
shutdown behavior is part of the deployed release identity rather than an
implicit platform default.

## Sanitized values

Every value is a variable with no default. No account id, ARN or domain appears
in any `.tf` file here.

## Modules used

| Module | Purpose |
| --- | --- |
| `iam-deployable-role` | One execution role per deployable. |
| `aurora-serverless-v2` | The finance cluster. |
| `sqs-queue` | Settlement work queue and its dead-letter queue. |
| `lambda-function` | The finance API. |
| `lambda-event-source` | Settlement consumer, with partial batch failures. |
| `ecs-task-oneshot` | The schema admin migration runner. |
| `eventbridge-scheduler` | Scheduled work. |

## Outputs

`finance_api_alias_arn`, `settlement_queue_arn`, `settlement_dlq_arn`,
`database_cluster_arn`, `schema_admin_task_definition_arn`, `role_arns`,
`schedule_arns`.

## Test

`tests/central-application.tftest.hcl` plans the root against a mock AWS
provider, with `mock_data "aws_s3_object"` supplying the artifact checksum the
Lambda module checks. It asserts one role per deployable, a published function
version, a dead-letter queue on the settlement queue, and that every schedule
target is one this root created.
