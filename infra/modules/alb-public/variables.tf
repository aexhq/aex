variable "name" {
  type        = string
  description = "Load balancer name."

  validation {
    condition     = can(regex("^aex-[a-z0-9-]{3,28}$", var.name))
    error_message = "The name must start with `aex-` and be at most 32 characters."
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

variable "subnet_ids" {
  type        = list(string)
  description = "Public subnets the load balancer sits in."

  validation {
    condition     = length(var.subnet_ids) >= 2
    error_message = "A public load balancer needs at least two subnets in different availability zones."
  }
}

variable "security_group_ids" {
  type        = list(string)
  description = "Security groups attached to the load balancer."

  validation {
    condition     = length(var.security_group_ids) > 0
    error_message = "At least one security group is required."
  }
}

variable "certificate_arn" {
  type        = string
  description = "ACM certificate for the HTTPS listener."

  validation {
    condition     = can(regex("^arn:aws[a-z-]*:acm:[a-z0-9-]+:[0-9A-Za-z-]{1,64}:certificate/[0-9a-f-]+$", var.certificate_arn))
    error_message = "The certificate must be an ACM certificate ARN."
  }
}

variable "idle_timeout" {
  type        = number
  default     = 1200
  description = "Idle timeout in seconds. The floor is 1200 because a streaming turn can legitimately hold a connection open with no bytes flowing for a long time, and a shorter timeout cuts the response mid-stream."

  validation {
    condition     = var.idle_timeout >= 1200 && var.idle_timeout <= 4000
    error_message = "The idle timeout must be at least 1200 seconds so a long streaming turn is not cut short."
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

variable "deregistration_delay" {
  type        = number
  default     = 30
  description = "Seconds a deregistering target keeps draining."

  validation {
    condition     = var.deregistration_delay >= 30 && var.deregistration_delay <= 3600
    error_message = "The deregistration delay must be at least 30 seconds."
  }
}

variable "forward_path_patterns" {
  type        = list(string)
  default     = ["/api/*"]
  description = "The only paths the public listener forwards. Everything else, `/internal/*` above all, gets the listener default action."

  validation {
    condition     = length(var.forward_path_patterns) > 0
    error_message = "At least one forwarded path pattern is required."
  }

  validation {
    condition     = alltrue([for p in var.forward_path_patterns : startswith(p, "/api/")])
    error_message = "The public listener may only forward paths under `/api/`."
  }

  validation {
    condition     = alltrue([for p in var.forward_path_patterns : !startswith(p, "/internal")])
    error_message = "`/internal/*` must never be reachable from the public listener."
  }
}

variable "access_logs_bucket" {
  type        = string
  description = "Bucket that receives access logs. Access logging is mandatory, so there is no null case."

  validation {
    condition     = can(regex("^[a-z0-9][a-z0-9.-]{2,62}$", var.access_logs_bucket))
    error_message = "The access log bucket must be a valid S3 bucket name; access logging is mandatory."
  }
}

variable "access_logs_prefix" {
  type        = string
  default     = "alb"
  description = "Key prefix for access logs."

  validation {
    condition     = can(regex("^[A-Za-z0-9!_.*'()/-]{1,256}$", var.access_logs_prefix))
    error_message = "The prefix must be a valid S3 key prefix."
  }
}

variable "ssl_policy" {
  type        = string
  default     = "ELBSecurityPolicy-TLS13-1-2-2021-06"
  description = "TLS policy on the HTTPS listener."

  validation {
    condition     = startswith(var.ssl_policy, "ELBSecurityPolicy-TLS13")
    error_message = "The TLS policy must be a TLS 1.3 policy."
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
  description = "Tags applied to the load balancer and target group."
}
