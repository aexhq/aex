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

# One target group, N rules pointing at it. The quotas that bite are per rule -
# `Condition Values per Rule` is 5 and `Condition Wildcards per Rule` is 6, both
# fixed - while `Rules per Application Load Balancer` is 100 and adjustable. So
# a service whose public surface needs more than five patterns is not
# inexpressible; it just needs more than one rule.
#
# Every priority is required and explicit. Nothing here derives or
# auto-increments one: a silently chosen priority is exactly how one service
# claims a slot another service was using, and that is the failure this module
# exists to prevent.
variable "rules" {
  type = list(object({
    priority      = number
    path_patterns = list(string)
  }))
  description = "The listener rules that forward to this service's target group. Each carries its own explicit priority and its own pattern list. Lower priorities are evaluated first, so a narrower rule must hold a lower number than any rule that would also match."

  validation {
    condition     = length(var.rules) > 0
    error_message = "At least one listener rule is required; a target group with no rule receives nothing."
  }

  validation {
    condition     = alltrue([for r in var.rules : length(r.path_patterns) > 0])
    error_message = "Every rule must forward at least one path pattern."
  }

  validation {
    condition = alltrue([
      for r in var.rules : alltrue([for p in r.path_patterns : startswith(p, "/api/")])
    ])
    error_message = "The public listener may only forward paths under `/api/`."
  }

  validation {
    condition = alltrue([
      for r in var.rules : alltrue([for p in r.path_patterns : !startswith(p, "/internal")])
    ])
    error_message = "`/internal/*` must never be reachable from the public listener."
  }

  # `Condition Values per Rule` is a hard AWS quota of 5, per rule.
  validation {
    condition     = alltrue([for r in var.rules : length(r.path_patterns) <= 5])
    error_message = "At most five path patterns fit one listener rule; `Condition Values per Rule` is a hard AWS quota of 5 and cannot be raised. Split the patterns across more rules in this list instead."
  }

  # `Condition Wildcards per Rule` is a hard AWS quota of 6, per rule, counting
  # every `*` and `?` across that rule's patterns.
  validation {
    condition = alltrue([
      for r in var.rules :
      sum([for p in r.path_patterns : length(replace(p, "/[^*?]/", ""))]) <= 6
      if length(r.path_patterns) > 0
    ])
    error_message = "At most six wildcard characters fit one listener rule; `Condition Wildcards per Rule` is a hard AWS quota of 6 and cannot be raised. Split the patterns across more rules in this list instead."
  }

  validation {
    condition     = alltrue([for r in var.rules : r.priority >= 1 && r.priority <= 50000])
    error_message = "Every rule priority must be between 1 and 50000."
  }

  validation {
    condition     = alltrue([for r in var.rules : r.priority == floor(r.priority)])
    error_message = "Every rule priority must be a whole number."
  }

  validation {
    condition     = length(distinct([for r in var.rules : r.priority])) == length(var.rules)
    error_message = "Two rules in this service target share a priority. A listener rejects a duplicate priority at apply; catching it here keeps the collision out of the plane."
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
