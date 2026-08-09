variable "name" {
  type        = string
  description = "Service target name. The target group is `<name>-tg`, so the bound is three characters tighter than the load balancer's."

  validation {
    condition     = can(regex("^aex-[a-z0-9-]{3,25}$", var.name))
    error_message = "The name must start with `aex-` and be at most 29 characters, so `<name>-tg` stays inside the 32-character target group name limit."
  }
}

variable "listener_arn" {
  type        = string
  description = "HTTPS listener this service attaches its one rule to. It comes from `alb-public`, which owns the listener and its fixed-response default."

  validation {
    condition     = can(regex("^arn:aws[a-z-]*:elasticloadbalancing:[a-z0-9-]+:[0-9A-Za-z-]{1,64}:listener/app/[A-Za-z0-9-]+/[0-9a-f]+/[0-9a-f]+$", var.listener_arn))
    error_message = "The listener must be an Application Load Balancer listener ARN."
  }
}

variable "vpc_id" {
  type        = string
  description = "VPC the target group lives in."

  validation {
    condition     = can(regex("^vpc-[0-9a-f]{8,32}$", var.vpc_id))
    error_message = "The VPC must be a VPC id."
  }
}

variable "target_port" {
  type        = number
  description = "Port the targets listen on."

  validation {
    condition     = var.target_port >= 1 && var.target_port <= 65535
    error_message = "The target port must be a valid port number."
  }
}

variable "health_check_path" {
  type        = string
  default     = "/internal/readyz"
  description = "Health check path. It is an internal path, which is exactly why the public listener must not be able to reach it."

  validation {
    condition     = startswith(var.health_check_path, "/")
    error_message = "The health check path must be absolute."
  }

  validation {
    condition     = startswith(var.health_check_path, "/internal/")
    error_message = "The health check path must be an internal path; a readiness probe is not a public endpoint."
  }
}

# Required, with no default, on purpose. A default is what lets two services
# collide: both would take it, and the second apply would fail on a duplicate
# priority - or worse, succeed against a different listener and shadow the
# first. Making every caller state its own integer is what makes the set
# unique-able at all.
variable "priority" {
  type        = number
  description = "Listener rule priority. Required and explicit: a default is what lets a second service silently claim the first service's slot. Lower numbers are evaluated first."

  validation {
    condition     = var.priority >= 1 && var.priority <= 50000
    error_message = "The rule priority must be between 1 and 50000."
  }

  validation {
    condition     = var.priority == floor(var.priority)
    error_message = "The rule priority must be a whole number."
  }
}

variable "path_patterns" {
  type        = list(string)
  description = "The only paths this rule forwards. Everything the rule set does not match, `/internal/*` above all, falls through to the listener's fixed 404."

  validation {
    condition     = length(var.path_patterns) > 0
    error_message = "At least one forwarded path pattern is required."
  }

  validation {
    condition     = alltrue([for p in var.path_patterns : startswith(p, "/api/")])
    error_message = "The public listener may only forward paths under `/api/`."
  }

  validation {
    condition     = alltrue([for p in var.path_patterns : !startswith(p, "/internal")])
    error_message = "`/internal/*` must never be reachable from the public listener."
  }

  # AWS quota, not taste: `Condition Values per Rule` is 5 and is not
  # adjustable. A rule that needs more values than this is a routing split that
  # does not fit one rule, and it must fail here rather than at apply.
  validation {
    condition     = length(var.path_patterns) <= 5
    error_message = "At most five path patterns fit one listener rule; `Condition Values per Rule` is a hard AWS quota of 5 and cannot be raised."
  }

  # `Condition Wildcards per Rule` is 6 and is not adjustable either.
  validation {
    condition = (
      length(var.path_patterns) == 0
      || sum([for p in var.path_patterns : length(replace(p, "/[^*?]/", ""))]) <= 6
    )
    error_message = "At most six wildcard characters fit one listener rule; `Condition Wildcards per Rule` is a hard AWS quota of 6 and cannot be raised."
  }
}

variable "deregistration_delay" {
  type        = number
  default     = 30
  description = "Seconds a deregistering target keeps draining."

  validation {
    condition     = var.deregistration_delay >= 30 && var.deregistration_delay <= 3600
    error_message = "The deregistration delay must be at least 30 seconds."
  }
}

variable "health_check" {
  type = object({
    interval            = number
    timeout             = number
    healthy_threshold   = number
    unhealthy_threshold = number
    matcher             = string
  })
  default = {
    interval            = 15
    timeout             = 5
    healthy_threshold   = 2
    unhealthy_threshold = 3
    matcher             = "200"
  }
  description = "Health check timing and expected status."

  validation {
    condition     = var.health_check.timeout < var.health_check.interval
    error_message = "The health check timeout must be shorter than the interval."
  }
}

variable "tags" {
  type        = map(string)
  default     = {}
  description = "Tags applied to the target group and the listener rule."
}
