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

## Every security group is created by the module that owns what it protects

This root used to take `alb_security_group_ids` and `service_security_group_ids`
and nothing anywhere created them. No module built the groups, and the private
deployment repository may not declare resources in an environment root, so the
groups these variables named could not exist. Both variables are gone.

`alb-public` creates the edge's group and admits TCP/443 and TCP/80 from the
internet. Each `ecs-service` creates its own task group - two groups, not one
shared between the deployables - admits the edge on its own container port, and
writes the matching egress on the edge's group for that same port. Neither
service can be reached through the other's rules.

What the region foundation still hands over is what the tasks are allowed to
reach: `interface_endpoint_security_group_id` and
`gateway_endpoint_prefix_list_ids`. Those are the whole of each task group's
egress - TLS to the shared interface endpoint group, TLS to the S3 and DynamoDB
prefix lists - and there is no `0.0.0.0/0` anywhere, which matters because the
regional VPC stands up no NAT gateway.

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
with the split, it just needs another rule. The stream service no longer needs
that: its whole surface is `/api/streams/*`, one condition value in one rule at
priority 10. The session API carries four prefixes across two rules, at 20 and
21, because `/api/sessions` and `/api/sessions/*` are two condition values.

## The regional surface splits on its first segment

`rules` is a required input on both services, and each one claims its routes by
first path segment. Nothing here depends on rule precedence.

It used to. `/api/sessions/{sessionId}/` was served by three services at once,
and the segment that told them apart came *after* a variable session id -
`/api/sessions/{sessionId}/events/stream` against
`/api/sessions/{sessionId}/messages`. No *prefix* separated them, so this root
routed only the part of the surface that did split and left `/api/sessions` out.
The workspace-scoped observability routes had the same shape one level up:
`/api/logs/query` was `regional-observation-api`'s and `/api/logs/stream` was
`regional-stream`'s, discriminated by the last segment.

The API surface was changed instead. Every regional route now carries a first
segment that names its owner, and `api/generated/registries/routes.json` is
where that is checked rather than asserted here:

| Prefix | Owner | Regional routes |
| --- | --- | --- |
| `/api/sessions/`, `/api/workspace/`, `/api/operations/`, `/api/billing/` | `regional-session-api` | 61 |
| `/api/streams/` | `regional-stream` | 24 |
| `/api/observations/` | `regional-observation-api` | 27 |
| `/api/secrets/` | `regional-secret-api` | 4 |
| `/api/otlp/` | `regional-otlp` | 3 |

Each prefix resolves to exactly one `servingArtifact`, so an ALB prefix match is
sufficient to route and the earlier workaround is retired. The anchored
two-wildcard patterns - `/api/sessions/*/events/*` and its five siblings - are no
longer needed, and neither is the priority ordering they would have required.
`operationId`s were deliberately left alone, so ids like
`observations_logs_stream` and `secret_put` no longer echo their URL. The
registry, not the path, is what binds an operation to its owner.

Ownership is per *operation*, not per path, so a path can still lose only part
of itself. `GET /api/workspace/secrets/{name}` is metadata and stays with
`regional-session-api`; the `PUT` and `DELETE` on that same template carry
plaintext and moved to `/api/secrets/{name}`. Nothing about the prefix property
depends on that: both first segments still have exactly one owner.

What is still *not* safe is matching on the trailing verb, as `/api/*/stream`
and `/api/*/listen`. An ALB `*` matches across `/`, and `regional-session-api`
serves `PUT /api/workspace/files/{name}` with a user-chosen name, so a file
called `stream` would be routed to the wrong service. That shape is rejected on
its merits, not on quota. The fixture asserts the inverse property directly: no
forwarded pattern may wildcard its own first segment.

`/api/sessions` and `/api/sessions/*` are two condition values, not one. An ALB
pattern ending in `/*` does not match the bare collection, and `POST
/api/sessions` and `GET /api/sessions` carry no session id.

What this root still does not settle is the other three artifacts.
`/api/observations/*`, `/api/secrets/*` and `/api/otlp/*` belong to
`regional-observation-api`, `regional-secret-api` and `regional-otlp`, and this
root deploys two services rather than five, so nothing here claims those three
prefixes. A request to them reaches the listener's fixed 404 default. That is
the honest outcome for a two-service root: the three prefixes are named here and
in the fixture's comments so their absence is a recorded gap rather than a
silent one, and no rule forwards them to a service that could not answer.

So the fixture in `tests/` routes: `/api/streams/*` to the stream service in one
rule, and `/api/workspace/*`, `/api/operations/*`, `/api/billing/*`,
`/api/sessions` and `/api/sessions/*` to the session API across two more.

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
that no first path segment is claimed by both services and no pattern wildcards
its own first segment, that the stream service's whole surface is the single
rule `/api/streams/*`, that the session API carries the reviewed Fargate shape,
and that its role is assumed by `ecs-tasks.amazonaws.com` rather than by Lambda.
