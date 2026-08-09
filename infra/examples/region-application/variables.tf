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

variable "interface_endpoint_security_group_id" {
  type        = string
  description = "The group every private AWS interface endpoint shares, from the region foundation. Each service's task group is given TLS egress to it."
}

variable "gateway_endpoint_prefix_list_ids" {
  type        = map(string)
  description = "Gateway endpoint service to AWS-managed prefix-list id, from the region foundation. Each service's task group is given TLS egress to `s3` and `dynamodb` through these."
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

variable "session_stream_api" {
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
  description = "The merged regional request-path service. `regional-session-api` and `regional-stream` were two Fargate services of identical shape, each with a production floor of two tasks; they are one deployable now, which makes that floor two tasks rather than four."

  validation {
    condition     = length(var.session_stream_api.rules) > 0
    error_message = "The service must declare the public paths it serves; the listener forwards nothing by default."
  }

  validation {
    condition     = length(distinct([for r in var.session_stream_api.rules : r.priority])) == length(var.session_stream_api.rules)
    error_message = "Each listener rule must claim its own priority. This root used to check the two services did not collide with each other; with one service the collision it has to rule out is within its own rule list."
  }

  validation {
    condition     = length(flatten([for r in var.session_stream_api.rules : r.path_patterns])) == length(distinct(flatten([for r in var.session_stream_api.rules : r.path_patterns])))
    error_message = "Two rules must not declare the same path pattern. Two rules matching one pattern is decided by priority alone, which makes the lower-priority rule silently unreachable on that path."
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

variable "alb" {
  type = object({
    name               = string
    certificate_arn    = string
    access_logs_bucket = string
  })
  description = "The public load balancer the regional request-path service attaches to."
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
