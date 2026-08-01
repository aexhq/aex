variable "plane" {
  type        = string
  description = "Deployment plane."
}

variable "region" {
  type        = string
  description = "AWS region."
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

variable "artifact_bucket" {
  type        = string
  description = "Bucket the publish lane wrote the Lambda ZIP to."
}

variable "cluster_name" {
  type        = string
  description = "ECS cluster name."
}

variable "session_api" {
  type = object({
    function_name           = string
    artifact_key            = string
    artifact_object_version = string
    artifact_sha256         = string
    memory_mb               = number
    timeout_s               = number
    log_retention_days      = number
    env                     = map(string)
  })
  description = "The regional session API Lambda."
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
    batch_size     = number
  })
  description = "The pipe that turns journal mutations into operation hints."
}

variable "stream_service" {
  type = object({
    name               = string
    image              = string
    cpu                = number
    memory             = number
    stop_timeout       = number
    container_port     = number
    log_group_name     = string
    log_retention_days = number
    execution_role_arn = string
    env                = map(string)
    autoscaling_bounds = object({
      min_capacity = number
      max_capacity = number
    })
    autoscaling_metrics = list(object({
      name         = string
      namespace    = string
      statistic    = string
      target_value = number
    }))
  })
  description = "The regional stream service."
}

variable "alb" {
  type = object({
    name               = string
    certificate_arn    = string
    access_logs_bucket = string
  })
  description = "The public load balancer in front of the stream service."
}

variable "deployable_grants" {
  type = map(object({
    assume_principal = object({
      type        = string
      identifiers = list(string)
    })
    wildcard_resource_allowlist = list(string)
    action_grants = list(object({
      sid              = string
      actions          = list(string)
      resources        = list(string)
      scopable         = bool
      condition_key    = optional(string)
      condition_values = optional(list(string))
    }))
  }))
  description = "Execution role grants per deployable."
}

variable "tags" {
  type        = map(string)
  description = "Tags applied to everything in the region application."
  default     = {}
}
