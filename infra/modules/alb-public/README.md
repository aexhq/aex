# `alb-public`

The public application load balancer in front of a regional service.

The idle timeout has a floor of 1200 seconds, because a streaming turn can hold
a connection open for a long time with no bytes moving and a shorter timeout
cuts the response mid-stream.

This module owns the load balancer, the two listeners and the access logs. It
owns no target group and no rule. Those live in `alb-service-target`, one
instance per service, and a root attaches each service to the `listener_arn`
this module exports.

The single-rule invariant did not go away; it moved and got sharper. It used to
be "this module creates one rule", which was only true while there was one
service. It is now "each `alb-service-target` creates exactly one rule at an
explicitly required priority", enforced per service. A priority with no default
is what keeps two services from silently claiming the same slot.

What keeps `/internal/*` private is unchanged and still lives here: the service
target groups health-check `/internal/readyz`, so the path exists and answers,
but no rule matches it and this listener's default action is a fixed 404.

## The edge owns its own ingress

This module creates the load balancer's security group. It used to take one, and
nothing anywhere created it: no module built these groups and the private
deployment repository may not declare resources in an environment root, so the
group a root was naming could not exist. The group is derived, not configurable
- `<name>-alb` - and it admits TCP/443 and TCP/80 from `0.0.0.0/0`. Port 80 is
open because this module already answers it with a permanent redirect; refusing
it would turn a plain `http://` request into a timeout rather than a 301.

`additional_security_group_ids` is added to that group, never substituted for
it. A caller can attach something extra; a caller cannot leave the public edge
admitting only what somebody wrote down elsewhere.

The group carries no egress rule of its own. Terraform revokes the allow-all
egress AWS attaches to a new group, and the only thing this load balancer needs
to reach is a service target. `ecs-service` writes that rule, on this group,
against its own task group and its own container port: the load balancer cannot
name a task group without depending on the service that already depends on it,
and one load balancer carries several services on different ports.

## Inputs

| Name | Type | Description |
| --- | --- | --- |
| `name` | `string` | Load balancer name, `aex-<...>`. |
| `vpc_id` | `string` | VPC the security group is created in. |
| `subnet_ids` | `list(string)` | At least two public subnets. |
| `additional_security_group_ids` | `list(string)` | Attached alongside the module's own group, never instead of it. Defaults to none. |
| `certificate_arn` | `string` | ACM certificate for the HTTPS listener. |
| `idle_timeout` | `number` | At least 1200 seconds. |
| `deregistration_delay` | `number` | At least 30 seconds; published to every service target. |
| `access_logs_bucket` | `string` | Mandatory access log bucket. |
| `access_logs_prefix` | `string` | Key prefix for access logs. |
| `ssl_policy` | `string` | TLS 1.3 policy. |
| `tags` | `map(string)` | Tags. |

## Outputs

| Name | Description |
| --- | --- |
| `dns_name` | DNS name of the load balancer. |
| `listener_arn` | ARN of the HTTPS listener, which every `alb-service-target` attaches to. |
| `security_group_id` | The load balancer's own group, which each service behind it names to admit exactly this edge. |
| `zone_id` | Hosted zone id, for an alias record. |
| `deregistration_delay` | Drain window, so a service behind it cannot disagree. |

## Policy asserted

- `idle_timeout` is at least 1200 seconds; a shorter value is rejected.
- This module creates no target group and no listener rule at all.
- The load balancer's security group is created here, in the given VPC, under a
  name derived from the load balancer's own name.
- Ingress is TCP/443 and TCP/80 from `0.0.0.0/0` and nothing else.
- The module's own group is always attached; an additional group is attached
  alongside it rather than in place of it.
- A `vpc_id` that is not a VPC, and an additional group that is not a security
  group, are both rejected. So are a name outside the `aex-` prefix, a single
  subnet, a certificate that is not an ACM certificate, and a TLS policy below
  1.3.
- The HTTPS listener's default action is a fixed 404, so anything no service
  rule matches - `/internal/*` included - is unreachable from the public
  listener.
- The HTTP listener permanently redirects to HTTPS.
- Access logs are enabled and point at the configured bucket; an empty bucket
  name is rejected.
- `deregistration_delay` is at least 30 seconds.

## Not asserted here

Idle-timeout behaviour under a real stream, header casing and body framing
parity between the load balancer and `lambda_http` are live-only.
