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
its own `alb-service-target`: one target group, and as many rules against it as
that service's pattern set needs, each at an explicitly required priority.
`regional-stream` is given the 10s and `regional-session-api` the 20s, so each
can add rules without reaching into the other's band. Lower priorities are
evaluated first, so a narrower rule must hold a lower number than any rule that
would also match.

The per-rule quotas are `Condition Values per Rule` = 5 and `Condition Wildcards
per Rule` = 6, neither adjustable; `Rules per Application Load Balancer` is 100
and *is* adjustable. So a pattern set too wide for one rule is not a problem
with the split, it just needs another rule. The stream service's six top-level
prefixes are six condition values, one more than a rule may carry, so this root
expresses them as two rules at priorities 10 and 11 against one target group.

## The public path split is expressible; one decision is still open

`rules` is a required input on both services, and this root deliberately routes
only the part of the surface that splits into disjoint patterns.

`api/generated/registries/routes.json` puts 12 of `regional-stream`'s 24 routes
and 22 of `regional-session-api`'s 61 under the same `/api/sessions/{sessionId}/`
prefix. The segment that tells them apart comes *after* a variable session id -
`/api/sessions/{sessionId}/events/stream` against
`/api/sessions/{sessionId}/messages` - so no *prefix* separates them.

Anchoring the discriminating segment does separate them safely.
`/api/sessions/*/events/*` cannot match `/api/sessions/{id}/messages` however
much the wildcards swallow, and no `regional-session-api` route contains
`events`, `logs`, `metrics`, `spans`, `telemetry` or `traces` in any position.
The six such patterns are two wildcards each, so they are two rules of three -
at the wildcard ceiling, and well inside the rule count.

What is *not* safe is matching on the trailing verb, as `/api/*/stream` and
`/api/*/listen`. An ALB `*` matches across `/`, and `regional-session-api`
serves `PUT /api/workspace/files/{name}` with a user-chosen name, so a file
called `stream` would be routed to the wrong service. That shape is rejected on
its merits, not on quota.

So the mechanism is in place, and what remains is a routing decision rather than
a limitation. Covering `/api/sessions/` means giving the stream service its
anchored session-scoped rules at low priorities and letting the session API hold
`/api/sessions` and `/api/sessions/*` behind them - correctness then rests on
rule *precedence* rather than on disjoint patterns, which is a property worth
choosing deliberately rather than inheriting. The alternatives are to split the
API surface so the two services stop sharing the prefix, or to give the
session-scoped stream routes their own top-level prefix.

Until that is decided, the fixture in `tests/` routes the cleanly separable
part: all six top-level stream prefixes across two rules, and
`/api/workspace/*`, `/api/operations/*` and `/api/billing/*` to the session API.
`/api/sessions` and everything below it are left out on purpose.

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
| `alb-service-target` | One target group per service, with its listener rules. |
| `ecs-service` | `regional-stream` and `regional-session-api`. |

## Outputs

`session_service_arn`, `operation_queue_arn`, `pipe_arn`, `stream_service_arn`,
`public_dns_name`, `role_arns`.

## Test

`tests/region-application.tftest.hcl` plans the root against a mock AWS
provider. It asserts one role per deployable, that the cluster and both log
groups are created here, that each service's drain window is the one its own
target group publishes, that both services' forwarded patterns are `/api/` paths
that do not collide, that the two services occupy disjoint listener priorities,
that the stream service expresses its six top-level prefixes as two rules, that
the session API carries the reviewed Fargate shape, and that its role is assumed
by `ecs-tasks.amazonaws.com` rather than by Lambda.
