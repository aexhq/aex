# `region-application`

The releasable half of one region: two Fargate request-path services -
`regional-stream` and `regional-session-api` - the operation queue and its
stream pipe, and the one public load balancer both services sit behind.

Everything here is replaced on a deployment. Nothing here holds state, which is
what makes it safe to plan and apply separately from `region-foundation`.

`regional-session-api` was a Lambda. It is a Fargate service now, the same
service class as `regional-stream`, carrying the same 1024/2048 task and the
same production floor of two tasks that `release/units.toml` records for it.

The drain window is wired rather than duplicated: each service's target group
publishes `deregistration_delay` and the service behind it consumes that same
value, so the load balancer and the task cannot disagree about how long a
deregistering target keeps serving.

The ECS cluster and both service log groups are created here rather than assumed
to exist, so a fresh region plans from an empty account.

## Two services, one listener

`alb-public` owns the load balancer and the listener whose default action is a
fixed 404. It owns no target group and no rule. Each service attaches through
its own `alb-service-target`, which is exactly one target group and one rule at
an explicitly required priority: `regional-stream` at 10, `regional-session-api`
at 20. Lower priorities are evaluated first, so the narrower pattern set takes
the lower number.

## The public path split is unresolved

`path_patterns` is a required input on both services and this root does not pick
a complete one, because on the evidence no complete prefix split exists.

`api/generated/registries/routes.json` puts 12 of `regional-stream`'s 24 routes
and 22 of `regional-session-api`'s 61 under the same `/api/sessions/{sessionId}/`
prefix. The segment that tells them apart comes *after* a variable session id -
`/api/sessions/{sessionId}/events/stream` against
`/api/sessions/{sessionId}/messages` - so no prefix separates them.

Anchoring the discriminating segment instead, as
`/api/sessions/*/events/*` and five more, needs 12 pattern values in one rule.
`Condition Values per Rule` is 5 and `Condition Wildcards per Rule` is 6; both
are hard AWS quotas that cannot be raised, and `alb-service-target` rejects a
pattern list that exceeds them. Even `regional-stream`'s six top-level nouns -
`events`, `logs`, `metrics`, `spans`, `telemetry`, `traces` - are six values and
so do not fit one rule on their own.

The one shape that does fit, matching on the trailing verb as `/api/*/stream`
and `/api/*/listen`, is unsafe: an ALB `*` matches across `/`, and
`regional-session-api` serves `PUT /api/workspace/files/{name}` with a
user-chosen name, so a file called `stream` would be routed to the wrong
service.

The fixture in `tests/` therefore routes exactly the part that does split
cleanly - `/api/events/*`, `/api/logs/*`, `/api/metrics/*`, `/api/spans/*` and
`/api/traces/*` to the stream service, `/api/workspace/*`, `/api/operations/*`
and `/api/billing/*` to the session API - and deliberately leaves out
`/api/telemetry/*`, `/api/sessions` and everything under
`/api/sessions/{sessionId}/`. Resolving those needs a decision this root cannot
make for itself: split the API surface so the two services no longer share the
`/api/sessions/` prefix, put a router in front, or give the session-scoped
stream routes their own top-level prefix.

## Sanitized values

Every value is a variable with no default. No account id, ARN or domain appears
in any `.tf` file here.

## Modules used

| Module | Purpose |
| --- | --- |
| `iam-deployable-role` | One execution role per deployable. |
| `sqs-queue` | Session operation queue and its dead-letter queue. |
| `dynamodb-stream-pipe` | Journal mutations to operation hints. |
| `ecs-cluster` | The cluster both services run in. |
| `log-group` | One group per service. |
| `alb-public` | Public edge: load balancer, listeners, fixed 404 default. |
| `alb-service-target` | One target group and one rule per service. |
| `ecs-service` | `regional-stream` and `regional-session-api`. |

## Outputs

`session_service_arn`, `operation_queue_arn`, `pipe_arn`, `stream_service_arn`,
`public_dns_name`, `role_arns`.

## Test

`tests/region-application.tftest.hcl` plans the root against a mock AWS
provider. It asserts one role per deployable, that the cluster and both log
groups are created here, that each service's drain window is the one its own
target group publishes, that both services' forwarded patterns are `/api/` paths
that do not collide, that the session API carries the reviewed Fargate shape,
and that its role is assumed by `ecs-tasks.amazonaws.com` rather than by Lambda.
