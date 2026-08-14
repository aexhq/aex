variable "plane" { type = string }
variable "region" { type = string }
variable "permissions_boundary_policy_arn" { type = string }
variable "vpc_id" { type = string }
variable "private_subnet_ids" { type = list(string) }
variable "public_subnet_ids" { type = list(string) }
variable "interface_endpoint_security_group_id" { type = string }
variable "gateway_endpoint_prefix_list_ids" { type = map(string) }
variable "kms_key_arn" { type = string }
variable "cluster_name" { type = string }
variable "service_discovery_namespace" { type = string }
variable "artifact_bucket" { type = string }

variable "alb" {
  type = object({ name = string, certificate_arn = string, access_logs_bucket = string })
}

variable "session_rules" {
  type = list(object({ priority = number, path_patterns = list(string) }))
  validation {
    condition     = length(var.session_rules) > 0 && alltrue([for path in flatten([for rule in var.session_rules : rule.path_patterns]) : startswith(path, "/api/")])
    error_message = "The session API must own at least one public `/api/` listener path."
  }
}

variable "services" {
  type = map(object({
    image                             = string
    cpu                               = number
    memory                            = number
    desired_count                     = number
    stop_timeout                      = number
    container_port                    = number
    health_check_grace_period_seconds = optional(number)
    log_group_name                    = string
    log_retention_days                = number
    execution_role_arn                = string
    env                               = map(string)
    autoscaling_bounds                = object({ min_capacity = number, max_capacity = number })
    autoscaling_metrics = list(object({
      name               = string
      namespace          = string
      statistic          = string
      target_value       = number
      dimensions         = optional(map(string), {})
      scale_out_cooldown = optional(number, 60)
      scale_in_cooldown  = optional(number, 300)
    }))
  }))

  validation {
    condition     = toset(keys(var.services)) == toset(["session-api", "brain-mux", "tool-mux"])
    error_message = "The regional service set is exactly session-api, brain-mux, and tool-mux."
  }
}

variable "lambda_units" {
  type = map(object({
    function_name           = string
    artifact_key            = string
    artifact_object_version = string
    artifact_sha256         = string
    memory_mb               = number
    timeout_s               = number
    reserved_concurrency    = number
    log_retention_days      = number
    env                     = map(string)
  }))

  validation {
    condition     = toset(keys(var.lambda_units)) == toset(["file-ingest-worker", "runtime-control-worker", "session-maintenance-worker"])
    error_message = "The regional Lambda set is exactly file ingest, runtime control, and session maintenance."
  }
}

variable "event_sources" {
  type = map(object({
    unit                = string
    source_arn          = string
    batch_size          = number
    max_batching_window = optional(number, 0)
    starting_position   = optional(string)
  }))
  validation {
    condition     = alltrue([for source in values(var.event_sources) : contains(keys(var.lambda_units), source.unit)])
    error_message = "Every regional event source must target one of the three focused workers."
  }
}

variable "deployable_grants" {
  type = map(object({
    assume_principal            = object({ type = string, identifiers = list(string) })
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

  validation {
    condition     = toset(keys(var.deployable_grants)) == toset(["session-api", "brain-mux", "tool-mux", "file-ingest-worker", "runtime-control-worker", "session-maintenance-worker"])
    error_message = "IAM roles must match the exact six regional/runtime placed-compute units."
  }
}

variable "tags" {
  type    = map(string)
  default = {}
}
