# `waf-rate-limit`

A per-source-IP rate limit in front of a named set of exact request paths, on one
Application Load Balancer.

## Why it exists

`http-api-v2` carried the only request throttle anywhere in the central plane:

```hcl
default_route_settings {
  throttling_burst_limit = 100
  throttling_rate_limit  = 50
}
```

API Gateway cannot integrate an ECS service. Moving the central HTTP surface onto
Fargate behind an ALB therefore removes that throttle, and an ALB has no
throttling of its own. This module is the replacement, and it is not a
like-for-like one — see [What it does not replace](#what-it-does-not-replace).

## What it is pointed at, and why not everything

Two routes are unauthenticated **by design**: `POST
/api/auth/device/authorizations` and `POST /api/auth/device/tokens`. On those,
every anonymous caller shares one replay principal, so two unrelated callers
presenting the same `Idempotency-Key` share one grant. `aex_central_http::router`
has carried a `TODO(cross-stream)` at that exact function saying the accepted
design puts a per-IP throttle in front of these two routes for that reason, and
that infrastructure owns the rule. This is that rule.

Everything else on the plane presents a credential, which the service resolves
against Aurora on every request now that the authorizer's 15 s result cache is
gone with the authorizer. A plane-wide limit would spend the same money
throttling traffic that is already accounted for per caller, and would put a
shared per-IP ceiling in front of customers behind one NAT.

## What it does not replace

| The gateway gave | This gives |
| --- | --- |
| `50 rps` sustained, `100` burst, **overall** | A per-source-IP count over a fixed window. There is no rate-based statement that expresses an aggregate rate. |
| A limit on every route | A limit on the paths named in `paths`, exactly. |
| Nothing else was needed | A web ACL is billed monthly whether or not it matches anything. |

## Cost

Roughly `$5`/month for the web ACL, `$1`/month for the rule, and `$0.60` per
million requests evaluated. The request charge applies to every request reaching
the load balancer, not only the ones the scope-down matches.

## Choices worth knowing about

**`default_action` is `allow`.** This ACL bounds one class of caller; it is not
the plane's admission control. A default block would make a mistake in this
module an outage of the whole listener, and every authorization decision already
belongs to the service behind it.

**`aggregate_key_type = "IP"`, not `FORWARDED_IP`.** The ALB sets
`X-Forwarded-For` from the connection it accepted, but a caller may append to it.
Keying on the forwarded header would let one source present a fresh key per
request and buy itself an unbounded rate.

**`positional_constraint = "EXACTLY"`, not `STARTS_WITH`.** A prefix on
`/api/auth/device/` would also cover any route added under it later. A route
inheriting a throttle nobody wrote for it is worse than a route with none,
because the limit is invisible at the point the route is authored.

**`sampled_requests_enabled = false` everywhere.** A sampled request is stored
with its headers, and the paths this rule watches carry device codes.

**No managed rule groups, no logging configuration.** Each is a separate decision
with a separate bill. A module that quietly enabled one would make the cost of
"add a rate limit" something other than what it says.

## Usage

```hcl
module "device_flow_rate_limit" {
  source = "../../modules/waf-rate-limit"

  name         = "aex-dev-central-device-flow"
  resource_arn = module.public_lb.arn
  rate_limit   = 100
  paths = [
    "/api/auth/device/authorizations",
    "/api/auth/device/tokens",
  ]
  tags = var.tags
}
```

## Alarming on it

`rule_metric_name` is the dimension `BlockedRequests` is published under.
Blocking is expected in small amounts; a sustained non-zero rate is either an
attack or a limit set too low, and the two are told apart by whether
`AllowedRequests` fell at the same time.
