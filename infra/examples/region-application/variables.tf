variable "plane" {
  type        = string
  description = "Deployment plane."
}

variable "region" {
  type        = string
  description = "AWS region."
}

variable "permissions_boundary_policy_arn" {
  type        = string
  description = "Owner-managed permissions boundary required on every deployable execution role."
}

variable "vpc_id" {
  type        = string
  description = "VPC from the region foundation."
}

variable "private_subnet_ids" {
  type        = list(string)
  description = "Private subnets services run in."
}

variable "public_subnet_ids" {
  type        = list(string)
  description = "Public subnets the load balancer sits in."
}

variable "service_security_group_ids" {
  type        = list(string)
  description = "Security groups attached to the service tasks."
}

variable "alb_security_group_ids" {
  type        = list(string)
  description = "Security groups attached to the load balancer."
}

variable "kms_key_arn" {
  type        = string
  description = "Customer-managed key for the operation queue."
}

variable "session_journal_stream_arn" {
  type        = string
  description = "Stream ARN of the session journal table, from the region foundation."
}

variable "cluster_name" {
  type        = string
  description = "ECS cluster name."
}

variable "session_api" {
  type = object({
    name                              = string
    image                             = string
    cpu                               = number
    memory                            = number
    desired_count                     = number
    stop_timeout                      = number
    container_port                    = number
    health_check_grace_period_seconds = number
    log_group_name                    = string
    log_retention_days                = number
    execution_role_arn                = string
    env                               = map(string)
    rules = list(object({
      priority      = number
      path_patterns = list(string)
    }))
    autoscaling_bounds = object({
      min_capacity = number
      max_capacity = number
    })
    autoscaling_metrics = list(object({
      name               = string
      namespace          = string
      statistic          = string
      target_value       = number
      dimensions         = optional(map(string), {})
      scale_out_cooldown = optional(number, 60)
      scale_in_cooldown  = optional(number, 300)
    }))
  })
  description = "The regional session API, a Fargate service behind the public load balancer. It was a Lambda; it is the same service class as `regional-stream` now, and carries the same shape."

  validation {
    condition     = length(var.session_api.rules) > 0
    error_message = "The session API must declare the public paths it serves; the listener forwards nothing by default."
  }

  validation {
    condition = length(setintersection(
      toset(flatten([for r in var.session_api.rules : r.path_patterns])),
      toset(flatten([for r in var.stream_service.rules : r.path_patterns])),
    )) == 0
    error_message = "The two services must not declare the same path pattern. Two rules matching one pattern is decided by priority alone, which makes the lower-priority service silently unreachable on that path."
  }

  validation {
    condition = length(setintersection(
      toset([for r in var.session_api.rules : r.priority]),
      toset([for r in var.stream_service.rules : r.priority]),
    )) == 0
    error_message = "The two services must not claim the same listener priority. Each module instantiation can only prove its own priorities unique; the overlap between them is this root's to check."
  }
}

variable "operation_queue" {
  type = object({
    name                      = string
    visibility_timeout        = number
    max_receive_count         = number
    message_retention_seconds = number
    dlq_retention_seconds     = number
  })
  description = "The session operation queue."
}

variable "stream_pipe" {
  type = object({
    name           = string
    filter_pattern = string
    input_template = string
    batch_size     = number
  })
  description = "The pipe that turns journal mutations into operation hints."
}

variable "stream_service" {
  type = object({
    name                              = string
    image                             = string
    cpu                               = number
    memory                            = number
    desired_count                     = number
    stop_timeout                      = number
    container_port                    = number
    health_check_grace_period_seconds = number
    log_group_name                    = string
    log_retention_days                = number
    execution_role_arn                = string
    env                               = map(string)
    rules = list(object({
      priority      = number
      path_patterns = list(string)
    }))
    autoscaling_bounds = object({
      min_capacity = number
      max_capacity = number
    })
    autoscaling_metrics = list(object({
      name               = string
      namespace          = string
      statistic          = string
      target_value       = number
      dimensions         = optional(map(string), {})
      scale_out_cooldown = optional(number, 60)
      scale_in_cooldown  = optional(number, 300)
    }))
  })
  description = "The regional stream service."

  validation {
    condition     = length(var.stream_service.rules) > 0
    error_message = "The stream service must declare the public paths it serves; the listener forwards nothing by default."
  }
}

variable "alb" {
  type = object({
    name               = string
    certificate_arn    = string
    access_logs_bucket = string
  })
  description = "The public load balancer both regional request-path services attach to."
}

variable "deployable_grants" {
  type = map(object({
    assume_principal = object({
      type        = string
      identifiers = list(string)
    })
    wildcard_resource_allowlist = list(string)
    action_grants = list(object({
      sid                = string
      actions            = list(string)
      resources          = list(string)
      scopable           = bool
      condition_operator = optional(string, "StringEquals")
      condition_key      = optional(string)
      condition_values   = optional(list(string))
    }))
  }))
  description = "Execution role grants per deployable."
}

variable "tags" {
  type        = map(string)
  description = "Tags applied to everything in the region application."
  default     = {}
}
