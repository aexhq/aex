# `region-application`

This example composes the stateless regional session application above the
separately managed regional foundations.

The public `session-api` ECS service owns the load-balancer routes. Private
`brain-mux` and `tool-mux` ECS services have separate roles, listeners, and
service-discovery names; Brain reaches Tool Mux only through short detached
start/read/cancel requests. Focused Lambda workers handle file ingest, runtime
control, and session maintenance from their exact DynamoDB stream sources.

Session admission freezes registered workspace file revisions and returns
before sandbox I/O. Preparation continues asynchronously. Tool Mux recovers the
same manifest on the first tool call when necessary, runs filesystem, Bash, and
both MCP transports inside the exact sandbox generation, and keeps large result
files local. Its customer-content authority is used only by explicit
`storage.persist`; Brain and session maintenance receive no content-bucket
binding. Session maintenance retains only telemetry cleanup authority.

Everything here is replaceable application compute. DynamoDB tables, buckets,
keys, queues, networking foundations, and other durable resources are inputs
owned by the foundation composition.

## Network and routing

`alb-public` creates the public listener with a fixed 404 default.
`alb-service-target` attaches only `session-api` to the accepted `/api/` route
patterns. Brain and Tool Mux are private services discovered through
`service-discovery`; their security groups expose no public listener. Brain has
the reviewed provider HTTPS egress. Tool Mux has no public egress because MCP
networking occurs inside the exact Hands guest generation.

Every service and worker receives its own `iam-deployable-role`. The example
does not synthesize cross-service storage grants: callers pass the exact action
grants and environment bindings for each deployable.

## Modules used

| Module | Purpose |
| --- | --- |
| `iam-deployable-role` | One execution role per service or worker. |
| `lambda-function` | Immutable aliases for the focused regional workers. |
| `lambda-event-source` | Exact DynamoDB stream consumer bindings. |
| `ecs-cluster` | Shared regional application cluster. |
| `service-discovery` | Private Brain and Tool Mux hostnames. |
| `log-group` | One log group per ECS service. |
| `alb-public` | Public session edge and fixed 404 default. |
| `alb-service-target` | Session API target group and listener rules. |
| `ecs-service` | Separate session-api, Brain Mux, and Tool Mux services. |

## Outputs

The root publishes `service_arns`, `function_alias_arns`,
`private_service_hostnames`, `public_dns_name`, and `role_arns`.

## Test

`tests/region-application.tftest.hcl` plans with a mock AWS provider. It pins the
three-service/three-worker topology, listener ownership, private discovery,
network and IAM separation, exact event sources, and the absence of automatic
customer-content authority from Brain and session maintenance.
