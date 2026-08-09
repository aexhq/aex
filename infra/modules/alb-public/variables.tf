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
  description = "VPC the load balancer's own security group is created in. It must be the VPC the public subnets belong to; a group is only meaningful inside one."

  validation {
    condition     = can(regex("^vpc-[0-9a-f]{8,17}$", var.vpc_id))
    error_message = "The VPC must be an EC2 VPC id such as `vpc-0123456789abcdef0`."
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

variable "additional_security_group_ids" {
  type        = list(string)
  default     = []
  description = "Groups attached to the load balancer alongside the one this module creates. The module's own group is always attached; anything here is added to it and never substituted for it, so no caller can leave the public edge with ingress nobody wrote down."

  validation {
    condition     = alltrue([for id in var.additional_security_group_ids : can(regex("^sg-[0-9a-f]{8,17}$", id))])
    error_message = "Every additional security group must be an EC2 security group id such as `sg-0123456789abcdef0`."
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

variable "deregistration_delay" {
  type        = number
  default     = 30
  description = "Seconds a deregistering target keeps draining. The target groups live in `alb-service-target` now, so this is the one drain window the load balancer publishes for every service attached to it; a root hands it to each service target and to the service behind it."

  validation {
    condition     = var.deregistration_delay >= 30 && var.deregistration_delay <= 3600
    error_message = "The deregistration delay must be at least 30 seconds."
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

variable "tags" {
  type        = map(string)
  default     = {}
  description = "Tags applied to the load balancer and its listeners."
}
