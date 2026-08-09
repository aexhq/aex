# `alb-service-target`

One service's attachment to the public load balancer: exactly one target group,
and as many listener rules pointing at it as that service's public surface
needs.

`alb-public` used to hold the target group and the rule itself, which made the
listener single-tenant - the module could describe one service and no more. That
shape was written when there was one service. Moving them here makes the
attachment per service: one instance of this module is one service's whole
attachment, and the listener can carry several services.

What replaces the old single-rule guarantee is `priority`, required on every
element of `rules` and defaulted nowhere. A default is precisely what would let
two services collide: both would take it, and the apply would either fail on a
duplicate priority or, worse, succeed and shadow the service that got there
first. Nothing here derives or auto-increments a priority; every one is stated
and reviewed.

`/internal/*` stays private the same way it always did. The target group
health-checks `/internal/readyz`, so the path exists and answers; no rule
matches it, and the listener's default action is a fixed 404.

## Why a list of rules

The quotas that bite are per rule, and the one that would bound the service is
not:

| Quota | Value | Adjustable |
| --- | --- | --- |
| `Condition Values per Rule` | 5 | no |
| `Condition Wildcards per Rule` | 6 | no |
| `Rules per Application Load Balancer` | 100 | yes |

So a service needing more than five patterns is not inexpressible - it needs
more than one rule. Six top-level prefixes are one rule of five and one of one.
Six anchored session-scoped patterns like `/api/sessions/*/events/*`, two
wildcards each, are two rules of three. Both stay far inside 100.

This module validates the two fixed quotas **per element**, so a rule that is
too wide fails at plan rather than at apply, and the fix is to split it across
another element rather than to give up on the pattern.

Rules are keyed in state by priority, which the validation proves unique within
the instantiation, so reordering the list does not move a rule's address.

## Inputs

| Name | Type | Description |
| --- | --- | --- |
| `name` | `string` | Target name, `aex-<...>`, at most 29 characters so `<name>-tg` fits the 32-character target group limit. |
| `listener_arn` | `string` | HTTPS listener from `alb-public`. |
| `vpc_id` | `string` | VPC for the target group. |
| `target_port` | `number` | Port the targets listen on. |
| `health_check_path` | `string` | Defaults to `/internal/readyz`; must be `/internal/`-prefixed. |
| `rules` | `list(object)` | `{ priority, path_patterns }` per rule. Priority required and explicit; patterns `/api/`-prefixed, at most 5 values and 6 wildcards **per rule**. |
| `deregistration_delay` | `number` | At least 30 seconds. |
| `health_check` | `object` | Interval, timeout, thresholds, matcher. |
| `tags` | `map(string)` | Tags. |

## Outputs

| Name | Description |
| --- | --- |
| `target_group_arn` | Target group the service registers with. |
| `deregistration_delay` | Drain window, so the service behind it cannot disagree. |
| `rule_priorities` | Priorities this service occupies, for a root to check against another service's. |

## Policy asserted

- Exactly one `aws_lb_target_group`, and one `aws_lb_listener_rule` per element
  of `rules`, each at that element's explicit priority.
- Every `priority` is required, whole, and within 1-50000; a fractional or
  out-of-range priority is rejected, and two elements sharing a priority are
  rejected.
- Forwarded patterns are all under `/api/`. A pattern of `/internal/*` or `/*`
  is rejected, and so is an empty rule list or a rule with no patterns.
- At most five path patterns and six wildcard characters **per rule**, matching
  the non-adjustable ALB rule quotas. The same patterns split across more rules
  are accepted.
- The health check path defaults to `/internal/readyz` and must be an internal
  path; a public path is rejected.
- `deregistration_delay` is at least 30 seconds and is reported as an output so
  the service and the target group cannot disagree.
- The target type is `ip`, because a Fargate task registers by address.

## Not asserted here

Priority uniqueness *across* module instances. Uniqueness within one
instantiation is validated here; two sibling module calls cannot see each other,
so the listener is what rejects a cross-service duplicate at apply. The roots
that instantiate this module are where the numbers are chosen and reviewed, and
`rule_priorities` is exported so a root can assert the sets are disjoint.

Whether a pattern set actually covers a service's routes. This module checks the
shape of the patterns, not that they match the API. The route registry is the
authority for that.
