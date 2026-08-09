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

## Inputs

| Name | Type | Description |
| --- | --- | --- |
| `name` | `string` | Load balancer name, `aex-<...>`. |
| `subnet_ids` | `list(string)` | At least two public subnets. |
| `security_group_ids` | `list(string)` | Security groups. |
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
| `zone_id` | Hosted zone id, for an alias record. |
| `deregistration_delay` | Drain window, so a service behind it cannot disagree. |

## Policy asserted

- `idle_timeout` is at least 1200 seconds; a shorter value is rejected.
- This module creates no target group and no listener rule at all.
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
