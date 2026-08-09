# `alb-service-target`

One service's attachment to the public load balancer: exactly one target group
and exactly one listener rule.

`alb-public` used to hold the target group and the rule itself, which made the
listener single-tenant - the module could describe one service and no more. That
shape was written when there was one service. Moving the pair here keeps the
invariant and makes it per service: one instance of this module is one target
group and one rule, so "exactly one rule" is still true of every service, and
the listener can carry several.

What replaces the old single-rule guarantee is `priority`. It is required and
has no default, deliberately. A default is precisely what would let two services
collide: both would take it, and the apply would either fail on a duplicate
priority or, worse, succeed and shadow the service that got there first. Making
every caller state its own integer is what makes the set unique-able at all.

`/internal/*` stays private the same way it always did. The target group
health-checks `/internal/readyz`, so the path exists and answers; no rule
matches it, and the listener's default action is a fixed 404.

## The rule quota is load-bearing

`Condition Values per Rule` is 5 and `Condition Wildcards per Rule` is 6. Both
are hard AWS quotas that cannot be raised. This module validates both, so a
routing split that does not fit inside one rule fails at plan instead of at
apply. A service whose public surface needs more than five patterns is telling
you the split is wrong, not that the validation is.

## Inputs

| Name | Type | Description |
| --- | --- | --- |
| `name` | `string` | Target name, `aex-<...>`, at most 29 characters so `<name>-tg` fits the 32-character target group limit. |
| `listener_arn` | `string` | HTTPS listener from `alb-public`. |
| `vpc_id` | `string` | VPC for the target group. |
| `target_port` | `number` | Port the targets listen on. |
| `health_check_path` | `string` | Defaults to `/internal/readyz`; must be `/internal/`-prefixed. |
| `priority` | `number` | Required, explicit, 1-50000. No default. |
| `path_patterns` | `list(string)` | Forwarded paths; all `/api/`-prefixed, at most 5 values and 6 wildcards. |
| `deregistration_delay` | `number` | At least 30 seconds. |
| `health_check` | `object` | Interval, timeout, thresholds, matcher. |
| `tags` | `map(string)` | Tags. |

## Outputs

| Name | Description |
| --- | --- |
| `target_group_arn` | Target group the service registers with. |
| `deregistration_delay` | Drain window, so the service behind it cannot disagree. |

## Policy asserted

- Exactly one `aws_lb_target_group` and one `aws_lb_listener_rule`, at the
  caller's explicit priority.
- `priority` is required, whole, and within 1-50000; a fractional or
  out-of-range priority is rejected.
- Forwarded patterns are all under `/api/`. A pattern of `/internal/*` or `/*`
  is rejected, and so is an empty list.
- At most five path patterns and six wildcard characters, matching the
  non-adjustable ALB rule quotas.
- The health check path defaults to `/internal/readyz` and must be an internal
  path; a public path is rejected.
- `deregistration_delay` is at least 30 seconds and is reported as an output so
  the service and the target group cannot disagree.
- The target type is `ip`, because a Fargate task registers by address.

## Not asserted here

Priority uniqueness *across* module instances. Terraform cannot see two sibling
module calls from inside one of them; the listener rejects a duplicate priority
at apply, and the roots that instantiate this module are where the numbers are
chosen and reviewed.
