# `region-application`

The releasable half of one region: the session API Lambda, the operation queue
and its stream pipe, the regional stream service, and the public load balancer
in front of it.

Everything here is replaced on a deployment. Nothing here holds state, which is
what makes it safe to plan and apply separately from `region-foundation`.

The drain window is wired rather than duplicated: the ALB module's
`deregistration_delay` output feeds the service, so the load balancer and the
task cannot disagree about how long a deregistering target keeps serving.

The ECS cluster and the service log group are created here rather than assumed
to exist, so a fresh region plans from an empty account.

## Sanitized values

Every value is a variable with no default. No account id, ARN or domain appears
in any `.tf` file here.

## Modules used

| Module | Purpose |
| --- | --- |
| `iam-deployable-role` | One execution role per deployable. |
| `sqs-queue` | Session operation queue and its dead-letter queue. |
| `dynamodb-stream-pipe` | Journal mutations to operation hints. |
| `lambda-function` | The regional session API. |
| `alb-public` | Public edge, one rule, `/api/*` only. |
| `ecs-service` | The regional stream service. |

## Outputs

`session_api_alias_arn`, `operation_queue_arn`, `pipe_arn`,
`stream_service_arn`, `public_dns_name`, `role_arns`.

## Test

`tests/region-application.tftest.hcl` plans the root against a mock AWS
provider, with `mock_data "aws_s3_object"` supplying the artifact checksum. It
asserts one role per deployable, that the cluster and log group are created
here, that the drain window carried from the load balancer is at least 30
seconds, and that the service image is digest-pinned.
