# `alb-public`

The public application load balancer in front of a regional service.

Two decisions shape it. The idle timeout has a floor of 1200 seconds, because a
streaming turn can hold a connection open for a long time with no bytes moving
and a shorter timeout cuts the response mid-stream. And the listener has exactly
one rule.

That single rule is what keeps `/internal/*` private. The target group
health-checks `/internal/readyz`, so the path exists and answers; the public
listener simply has no rule that matches it, and its default action is a fixed
404. Adding a second rule is the failure mode this module is shaped to prevent.

## Inputs

| Name | Type | Description |
| --- | --- | --- |
| `name` | `string` | Load balancer name, `aex-<...>`. |
| `vpc_id` | `string` | VPC for the target group. |
| `subnet_ids` | `list(string)` | At least two public subnets. |
| `security_group_ids` | `list(string)` | Security groups. |
| `certificate_arn` | `string` | ACM certificate for the HTTPS listener. |
| `idle_timeout` | `number` | At least 1200 seconds. |
| `target_port` | `number` | Port the targets listen on. |
| `health_check_path` | `string` | Defaults to `/internal/readyz`. |
| `deregistration_delay` | `number` | At least 30 seconds. |
| `forward_path_patterns` | `list(string)` | Defaults to `["/api/*"]`. |
| `access_logs_bucket` | `string` | Mandatory access log bucket. |
| `access_logs_prefix` | `string` | Key prefix for access logs. |
| `ssl_policy` | `string` | TLS 1.3 policy. |
| `health_check` | `object` | Interval, timeout, thresholds, matcher. |
| `tags` | `map(string)` | Tags. |

## Outputs

| Name | Description |
| --- | --- |
| `dns_name` | DNS name of the load balancer. |
| `listener_arn` | ARN of the HTTPS listener. |
| `target_group_arn` | Target group services register with. |
| `zone_id` | Hosted zone id, for an alias record. |
| `deregistration_delay` | Drain window, so a service behind it cannot disagree. |

## Policy asserted

- `idle_timeout` is at least 1200 seconds; a shorter value is rejected.
- The health check path defaults to `/internal/readyz` and must be an internal
  path; a public path is rejected.
- There is exactly one `aws_lb_listener_rule`. It forwards only patterns under
  `/api/`, and a pattern of `/internal/*` or `/*` is rejected by validation.
- The HTTPS listener's default action is a fixed 404, so anything the single
  rule does not match - `/internal/*` included - is unreachable from the public
  listener.
- The HTTP listener permanently redirects to HTTPS.
- Access logs are enabled and point at the configured bucket; an empty bucket
  name is rejected.
- `deregistration_delay` is at least 30 seconds.

## Not asserted here

Idle-timeout behaviour under a real stream, header casing and body framing
parity between the load balancer and `lambda_http` are live-only.
