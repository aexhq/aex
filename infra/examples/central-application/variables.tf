variable "plane" {
  type        = string
  description = "Deployment plane."
}

variable "region" {
  type        = string
  description = "Region the central plane runs in."
}

variable "permissions_boundary_policy_arn" {
  type        = string
  description = "Owner-managed permissions boundary required on every deployable execution role."
}

variable "subnet_ids" {
  type        = list(string)
  description = "Private subnets from the region foundation."
}

variable "security_group_ids" {
  type        = list(string)
  description = "Security groups for the cluster and the database."
}

variable "kms_key_arn" {
  type        = string
  description = "Customer-managed key for the queue and the database."
}

variable "artifact_bucket" {
  type        = string
  description = "Bucket the publish lane wrote the Lambda ZIP to."
}

variable "finance_api" {
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
  description = "The finance API Lambda. The artifact was built, digested and published long before this plan runs."
}

variable "settlement_queue" {
  type = object({
    name                      = string
    visibility_timeout        = number
    max_receive_count         = number
    message_retention_seconds = number
    dlq_retention_seconds     = number
    batch_size                = number
  })
  description = "The settlement work queue and its consumer batching."
}

variable "database" {
  type = object({
    cluster_identifier    = string
    database_name         = string
    master_username       = string
    engine_version        = string
    min_acu               = number
    max_acu               = number
    backup_retention_days = number
  })
  description = "The central finance cluster."
}

variable "schema_admin" {
  type = object({
    family             = string
    image              = string
    cpu                = number
    memory             = number
    stop_timeout       = number
    log_group_name     = string
    secret_env         = map(string)
    execution_role_arn = string
  })
  description = "The one-shot schema admin task. The image is digest-pinned and is itself the central migration-bytes identity."
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
  description = "Execution role grants per deployable, taken from the generated IAM action lists."
}

variable "schedules" {
  type = list(object({
    name                    = string
    description             = string
    expression              = string
    flexible_window_minutes = number
    target_kind             = string
    input                   = string
    role_arn                = string
  }))
  description = "Scheduled work. `target_kind` selects which deployable this root points the schedule at, so the root never has to name a mutable target."
}

variable "schedule_group_name" {
  type        = string
  description = "Schedule group name."
}

variable "tags" {
  type        = map(string)
  description = "Tags applied to everything in the central application."
  default     = {}
}
