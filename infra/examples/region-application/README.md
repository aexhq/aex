# `region-application`

The releasable half of one region: the public Fargate request-path service
`session-stream-api`, the operation queue and its stream pipe, and the one
public load balancer used only by the request path. Tool Mux runs in-process
with Brain and has no second service, role, listener, or discovery name here.

Everything here is replaced on a deployment. Nothing here holds state, which is
what makes it safe to plan and apply separately from `region-foundation`.

`session-stream-api` carries the current regional request surface in one
1024/2048 task with the production floor of two tasks recorded in
`release/units.toml`. Despite its retained deployable name, the launch contract
is unary: the retired NDJSON stream, observation-query, and customer-ingestion
routes are absent rather than shipped dormant.

The drain window is wired rather than duplicated: each service's target group
publishes `deregistration_delay` and the service behind it consumes that same
value, so the load balancer and the task cannot disagree about how long a
deregistering target keeps serving.

The ECS cluster and service log group are created here rather than assumed to
exist, so a fresh region plans from an empty account.

## Every security group is created by the module that owns what it protects

This root used to take `alb_security_group_ids` and `service_security_group_ids`
and nothing anywhere created them. No module built the groups, and the private
deployment repository may not declare resources in an environment root, so the
groups these variables named could not exist. Both variables are gone.

`alb-public` creates the edge's group and admits TCP/443 and TCP/80 from the
internet. `ecs-service` creates the task group, admits the edge on its own
container port, and writes the matching egress on the edge's group for that
same port.

What the region foundation still hands over is what the tasks are allowed to
reach: `interface_endpoint_security_group_id` and
`gateway_endpoint_prefix_list_ids`. Those are the whole of each task group's
egress - TLS to the shared interface endpoint group, TLS to the S3 and DynamoDB
prefix lists - and there is no `0.0.0.0/0` anywhere, which matters because the
regional VPC stands up no NAT gateway.

## One service, one listener

`alb-public` owns the load balancer and the listener whose default action is a
fixed 404. It owns no target group and no rule. The service attaches through
`alb-service-target`: one target group, and as many rules against it as its
pattern set needs, each at an explicitly required priority. The current rules
occupy the 20 band. Lower priorities are evaluated first, so a narrower rule
must hold a lower number than any rule that would also match.

The per-rule quotas are `Condition Values per Rule` = 5 and `Condition Wildcards
per Rule` = 6, neither adjustable; `Rules per Application Load Balancer` is 100
and *is* adjustable. The current seven patterns therefore occupy two rules.
Bare `/api/sessions`, `/api/workspace`, and `/api/operations` each need an
explicit value because an ALB pattern ending in `/*` does not match the bare
collection.

## The regional surface splits on its first segment

`rules` is a required input, and the service claims every current regional
route by a literal first path segment. Nothing here depends on rule precedence.
`api/generated/registries/routes.json` is the authority:

| Prefix | Owner | Regional routes |
| --- | --- | --- |
| `/api/sessions/`, `/api/workspace/`, `/api/operations/`, `/api/billing/` | `session-stream-api` | 43 |

All four prefixes resolve to `session-stream-api`, so the rules divide only at
the five-condition quota. The fixture routes bare and nested workspace and
operations paths plus `/api/billing/*` in one rule, then bare and nested session
paths in the other. The listener's fixed 404 default honestly rejects every
retired or unknown prefix.

## Sanitized values

Every value is a variable with no default. No account id, ARN or domain appears
in any `.tf` file here.

## Modules used

| Module | Purpose |
| --- | --- |
| `iam-deployable-role` | One execution role per deployable. |
| `sqs-queue` | Session operation queue and its dead-letter queue. |
| `dynamodb-stream-pipe` | Journal mutations to operation hints. |
| `ecs-cluster` | The regional application cluster. |
| `log-group` | One group per service. |
| `alb-public` | Public edge: load balancer, listeners, fixed 404 default. |
| `alb-service-target` | One target group, with its listener rules. |
| `ecs-service` | `session-stream-api` behind the public target. |

## Outputs

`session_stream_service_arn`, `operation_queue_arn`, `pipe_arn`,
`public_dns_name`, `role_arns`.

## Test

`tests/region-application.tftest.hcl` plans the root against a mock AWS
provider. It asserts one role for the regional service; that the cluster and
log group are created here; that the request-path service's drain window is the one its
target group publishes; that its forwarded patterns are `/api/` paths that do
not collide; that no pattern wildcards its own first segment; that the two rules
claim exactly the four current regional first segments; that the session API
carries the reviewed Fargate shape; and that its role is assumed by
`ecs-tasks.amazonaws.com` rather than by Lambda.
