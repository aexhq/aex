variable "plane" {
  type        = string
  description = "Plane label for a self-host deployment. Use `prd` unless you are running a second, disposable environment."
}

variable "region" {
  type        = string
  description = "The one region everything runs in."
}

variable "account_id" {
  type        = string
  description = "Account the deployment lives in."
}

variable "name_prefix" {
  type        = string
  description = "Physical name prefix for the regional tables."
}

variable "bucket_suffix" {
  type        = string
  description = "Suffix that makes the bucket names globally unique."
}

variable "vpc" {
  type = object({
    name               = string
    cidr               = string
    az_count           = number
    availability_zones = list(string)
    endpoints          = list(string)
  })
  description = "Network shape."
}

variable "authority_keys" {
  type = map(object({
    alias                     = string
    description               = string
    encryption_context_equals = map(string)
    policy_statements = list(object({
      sid                          = string
      effect                       = string
      principal_type               = string
      principals                   = list(string)
      actions                      = list(string)
      resources                    = list(string)
      data_plane                   = bool
      encryption_context_workspace = optional(string)
    }))
  }))
  description = "One customer-managed key per authority."
}

variable "table_definitions" {
  type = list(object({
    logical_name                = string
    authority                   = string
    pinned_physical_name        = optional(string)
    hash_key                    = string
    range_key                   = optional(string)
    billing_mode                = string
    point_in_time_recovery_days = number
    deletion_protection         = bool
    ttl_attribute               = optional(string)
    stream_view_type            = optional(string)
    attributes = list(object({
      name = string
      type = string
    }))
    global_secondary_indexes = optional(list(object({
      name               = string
      hash_key           = string
      range_key          = optional(string)
      projection_type    = string
      non_key_attributes = optional(list(string))
    })), [])
  }))
  description = "Decoded contents of the published regional table bundle."
}

variable "table_definitions_digest" {
  type        = string
  description = "The `blake3:` digest the decoded bundle carries for its own definition set."
}

variable "expected_definitions_digest" {
  type        = string
  description = "The same digest, as the release manifest pins it."
}

variable "content_bucket_purpose" {
  type        = string
  description = "What the content store holds; the last component of `aex-<plane>-<region>-<purpose>`."
}

variable "content_authority" {
  type        = string
  description = "Which authority key encrypts the content bucket."
  default     = "content"
}

variable "content_lifecycle_role_arn" {
  type        = string
  description = "The only principal permitted to delete a content object."
}

variable "artifact_retention_days" {
  type        = number
  description = "Days a superseded infrastructure artifact version is kept."
}

variable "ops_topic_name" {
  type        = string
  description = "Name of the operational notification topic."
}

variable "ops_publish_principals" {
  type = list(object({
    type       = string
    identifier = string
  }))
  description = "Explicit publishers on the ops topic."
}

variable "ops_subscriptions" {
  type = list(object({
    protocol = string
    endpoint = string
  }))
  description = "Where operational notifications go."
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
  description = "The one Lambda this example deploys, as a worked instance of the release contract."
}

variable "session_api_grants" {
  type = object({
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
  })
  description = "Execution role grants for the session API."
}

variable "tags" {
  type        = map(string)
  description = "Tags applied to everything."
  default     = {}
}
