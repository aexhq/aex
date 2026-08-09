variable "name" {
  type        = string
  description = "The web ACL name. Also the metric name prefix, so it is what an alarm is written against."

  validation {
    condition     = can(regex("^aex-[a-z0-9-]{3,60}$", var.name))
    error_message = "The name must start with `aex-` so a console listing groups this plane's ACLs together, and hold only lowercase letters, digits and hyphens."
  }
}

variable "resource_arn" {
  type        = string
  description = "The Application Load Balancer this web ACL is associated with."

  validation {
    condition     = can(regex("^arn:aws[a-z-]*:elasticloadbalancing:[a-z0-9-]+:[0-9A-Za-z-]{1,64}:loadbalancer/app/[A-Za-z0-9-]+/[0-9a-f]+$", var.resource_arn))
    error_message = "The associated resource must be an Application Load Balancer ARN. A REGIONAL web ACL cannot be attached to a CloudFront distribution, and attaching one to a listener or a target group is not a thing AWS accepts."
  }
}

variable "paths" {
  type        = list(string)
  description = <<-EOT
    The exact request paths the rate limit applies to.

    Exact paths rather than prefixes, and a scope-down rather than a plane-wide
    limit. The two unauthenticated device-flow routes are the ones with no other
    control on them: every anonymous caller shares one replay principal, so two
    unrelated callers presenting the same `Idempotency-Key` share one grant. A
    plane-wide limit would instead throttle authenticated callers whose
    credentials are already resolved per request, which is the traffic the
    platform is paid for.
  EOT

  validation {
    condition     = length(var.paths) > 0
    error_message = "At least one path is required; a rate-based rule scoped down to nothing matches nothing and the web ACL is then a monthly charge that protects no route."
  }

  validation {
    condition     = alltrue([for p in var.paths : startswith(p, "/api/")])
    error_message = "Every scoped path must be a public API path under `/api/`. `/internal/*` is not reachable from the public listener, so rate limiting it would be protecting a path no caller can address."
  }

  validation {
    condition     = alltrue([for p in var.paths : !strcontains(p, "*") && !strcontains(p, "?")])
    error_message = "Paths are matched exactly, so a wildcard would silently match nothing. Name each route in full."
  }

  # `Scope-down statement` nesting is bounded, and the OR below holds one
  # byte-match per path.
  validation {
    condition     = length(var.paths) <= 10
    error_message = "At most ten paths fit one scope-down statement here. More than that is a plane-wide limit wearing a disguise; state it as one."
  }

  validation {
    condition     = length(distinct(var.paths)) == length(var.paths)
    error_message = "A path is repeated. Two identical byte-match statements in one OR is a rule that costs twice and matches once."
  }
}

variable "rate_limit" {
  type        = number
  description = "Requests from one source IP, over the evaluation window, before the rule blocks."

  validation {
    condition     = var.rate_limit >= 10 && var.rate_limit <= 2000000000
    error_message = "The rate limit must be between 10 and 2000000000; AWS refuses anything below 10."
  }

  validation {
    condition     = var.rate_limit == floor(var.rate_limit)
    error_message = "The rate limit must be a whole number of requests."
  }
}

variable "evaluation_window_seconds" {
  type        = number
  default     = 300
  description = "The window the rate is measured over. AWS admits exactly 60, 120, 300 and 600."

  validation {
    condition     = contains([60, 120, 300, 600], var.evaluation_window_seconds)
    error_message = "The evaluation window must be 60, 120, 300 or 600 seconds; AWS accepts no other value and rejects the rest at apply."
  }
}

variable "tags" {
  type        = map(string)
  default     = {}
  description = "Tags applied to the web ACL."
}
