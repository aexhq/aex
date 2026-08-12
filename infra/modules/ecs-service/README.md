# `ecs-service`

A long-running Fargate service: `brain-mux` or `session-stream-api`.

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

## Behind a load balancer

A service registered with a target group must state
`health_check_grace_period_seconds`. ECS starts counting load balancer
health-check failures the moment a task reaches RUNNING, and this module runs
with `wait_for_steady_state` and the deployment circuit breaker both on. Without
a grace period a service that needs longer than one unhealthy window to answer
its first probe does not deploy slowly - the apply fails and rolls back. The
variable is rejected on a service with no load balancer, because ECS rejects it
too.

A metric in an AWS-owned namespace must carry `dimensions`. Undimensioned,
`AWS/ApplicationELB` or `ECS/ContainerInsights` resolves to every load balancer
or cluster in the account aggregated into one series: the policy stays green and
scales this service on traffic that is not its own. Service-published namespaces
may omit them.

Each metric also carries `scale_out_cooldown` and `scale_in_cooldown`, 60 and
300 seconds by default. The asymmetry is deliberate and enforced: scale-in may
not be quicker than scale-out, because shedding capacity faster than it is added
is how a service oscillates under a load pattern it should have absorbed.

## The tasks' own security group

This module creates the group its tasks run with. It used to take one, and
nothing anywhere created it: no module built these groups and the private
deployment repository may not declare resources in an environment root, so the
group a root was naming could not exist. The name is derived from the plane- and
region-qualified `task_definition_family` rather than from `name`, because
`name` is bare - `session-stream-api` - and both planes live in one account.

Ingress is one rule or none. A service given `load_balancer_security_group_ids`
admits that group on `container_port`, and nothing else: not a CIDR, not a
subnet range. The health check arrives on the same port as the traffic, so there
is no second rule. A service with no edge in front of it admits nothing at all.

That variable is a list of at most one rather than a single id, and the reason
is mechanical. The load balancer's group is created in the same plan as the
service behind it, so its id is unknown while planning; a `count` written over
`id == null` cannot be planned and the root fails outright with *the count value
depends on resource attributes that cannot be determined until apply*. The
length of a one-element list is known even when the element is not.

The same module writes the matching egress rule **on the load balancer's
group**, because that is the only place both ends are in scope. `alb-public`
creates that group but cannot reference a task group without depending on the
service that already depends on it, and one load balancer carries several
services on different ports. Terraform revokes the allow-all egress AWS attaches
to a new group, so without this rule the edge reaches nothing and every target
reads unhealthy while the service, the target group and the listener all look
correct.

Egress from the tasks is three rules and no `0.0.0.0/0`:

- TCP/443 to `interface_endpoint_security_group_id`. Every interface endpoint in
  the VPC shares that one group, so one rule covers ECR, CloudWatch Logs, KMS,
  Secrets Manager, STS and SQS together;
- TCP/443 to the `s3` prefix list, for the image layers a task starts from;
- TCP/443 to the `dynamodb` prefix list, for the journal.

A gateway endpoint is a route table entry rather than an interface, so it has no
group to reference and its AWS-managed prefix list is the only way an egress
rule can name it. The regional VPC stands up no NAT gateway, so anything not
named here is unreachable rather than merely unauthorised. Task role credentials
need no rule: they arrive over the link-local task metadata address, which no
security group filters.

`additional_security_group_ids` is added to the module's group, never
substituted for it.

## Inputs

| Name | Type | Description |
| --- | --- | --- |
| `name` | `string` | Service name; also selects the `brain-mux` and `session-stream-api` pins. |
| `task_definition_family` | `string` | Plane- and region-qualified family, `aex-<dev\|prd>-<region>-<service-name>`. |
| `cluster_arn` / `cluster_name` | `string` | Cluster the service runs in. |
| `image` | `string` | Digest-pinned image. |
| `cpu` / `memory` | `number` | Fargate task size. |
| `runtime_platform` | `object` | Explicit architecture and OS family. |
| `desired_count` | `number` | Task count; defaults to 1. At least 2 for production `brain-mux`, exactly 1 for development `brain-mux`. |
| `stop_timeout` | `number` | Drain window: 120 for `brain-mux`, 30 for `session-stream-api`. |
| `deregistration_delay` | `number` | Target-group drain window; at least 30. |
| `health_check_grace_period_seconds` | `number` | Required behind a load balancer, rejected without one. |
| `circuit_breaker` | `object` | `{ enable = true, rollback = false }`; the breaker stops unhealthy deployments and releases fix forward. |
| `autoscaling_metrics` | `list(object)` | Target-tracking metrics with `dimensions` and both cooldowns, or `[]` for fixed-count mode. |
| `autoscaling_bounds` | `object` | `{ min_capacity, max_capacity }`; both equal `desired_count` in fixed-count mode. |
| `env` / `secret_env` | `map(string)` | Environment; secrets by ARN reference. |
| `container_port` | `number` | Port the container listens on. |
| `target_group_arn` | `string` | Optional target group to register with. |
| `task_role_arn` / `execution_role_arn` | `string` | Roles. |
| `subnets` | `list(string)` | Private subnets the tasks run in. |
| `vpc_id` | `string` | VPC the task security group is created in. |
| `interface_endpoint_security_group_id` | `string` | The group every interface endpoint shares; the tasks get TLS egress to it. |
| `gateway_endpoint_prefix_list_ids` | `map(string)` | Must carry `s3` and `dynamodb`; the tasks get TLS egress to each prefix list. |
| `load_balancer_security_group_ids` | `list(string)` | At most one, empty for a service with no edge. The only source the tasks admit. |
| `additional_security_group_ids` | `list(string)` | Attached alongside the module's own group, never instead of it. Defaults to none. |
| `log_group_name` / `region` | `string` | Logging. |
| `tags` | `map(string)` | Tags. |

## Outputs

| Name | Description |
| --- | --- |
| `service_arn` | ARN of the service. |
| `task_definition_arn` | Revisioned task definition ARN. |
| `security_group_id` | The tasks' own group, for a caller that has to write a rule against it. |
| `deregistration_delay` | Drain window for the target group in front of the service. |

## Policy asserted

- The `image` validation regex rejects any `:tag` reference; only
  `<repository>@sha256:<64 hex>` is accepted.
- The deployment circuit breaker is enabled and rolls back; disabling it is
  rejected.
- `desired_count` defaults to 1. A production `brain-mux` below two tasks is
  rejected, a development `brain-mux` other than one task is rejected, and for
  `brain-mux` any capacity bound that does not equal `desired_count` is rejected.
- `stop_timeout` is 120 for `brain-mux` and 30 for `session-stream-api`; any
  other value for those two services is rejected. The `session-stream-api` pin is
  load-bearing in both directions: the process derives its admitted
  `AEX_DRAIN_DEADLINE_MS` ceiling from the same 30 seconds and refuses to start
  on a deadline that could not fire before SIGKILL.
- A service with a `target_group_arn` must set
  `health_check_grace_period_seconds`, and a service without one may not.
- Non-empty autoscaling configuration must include a policy that tracks a
  service-published metric. A configuration that scales on `AWS/ECS` CPU alone
  is rejected.
- A metric in an `AWS/` or `ECS/` namespace must carry at least one dimension;
  an undimensioned one is rejected rather than silently scaling on the account
  aggregate.
- `scale_in_cooldown` may not be shorter than `scale_out_cooldown`.
- Empty autoscaling configuration creates no target or policy and is accepted
  only with capacity bounds collapsed to `desired_count`.
- `deregistration_delay` is at least 30 seconds.
- Tasks never receive a public address.
- Task-definition revisions are retained on replacement or destroy. Release roles therefore do not need the resource-unscopable ECS deregistration action; plane-aware revision cleanup is a separate operational responsibility.
- Every environment key is namespaced `AEX_*`, and every secret value is an ARN
  reference.
- `brain-mux` carries `AEX_MAX_ACTIVE_ACTIVATIONS = "16"`; an absent or different
  activation budget is rejected in either plane.
- The task security group is created here, in the given VPC, under a name
  derived from the plane-qualified task definition family.
- Task egress is exactly the interface endpoint group and the two gateway
  prefix lists, all TCP/443, and no rule names a CIDR.
- A load-balanced service admits exactly one source - the load balancer's group,
  on `container_port` - and a service with no load balancer admits nothing.
- The matching egress on the load balancer's group names this service's own task
  group and this service's own port.
- The module's own group is always attached; an additional group is attached
  alongside it rather than in place of it.
- A service with a `target_group_arn` must name a load balancer group and a
  service without one may not; more than one is rejected, as are a `vpc_id`, an
  interface endpoint group, an additional group or a prefix-list id of the wrong
  shape, and a gateway endpoint map missing `s3` or `dynamodb`.

## Not asserted here

Drain behaviour, deregistration timing and the real task-start interval are the
`aws.ecs.run_task` seam, owned by the `brain-mux` and `session-stream-api` live
suites.
