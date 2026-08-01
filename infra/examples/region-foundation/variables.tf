variable "plane" {
  type        = string
  description = "Deployment plane this region belongs to."
}

variable "region" {
  type        = string
  description = "AWS region."
}

variable "name_prefix" {
  type        = string
  description = "Physical name prefix for regional tables."
}

variable "vpc" {
  type = object({
    name               = string
    cidr               = string
    az_count           = number
    availability_zones = list(string)
    endpoints          = list(string)
  })
  description = "Regional network shape. Zone names are supplied because zone naming is per-account."
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
  description = "One customer-managed key per authority. Table encryption is resolved from a table's authority, so every authority a table names must appear here."
}

variable "table_definitions" {
  type = list(object({
    logical_name                = string
    authority                   = string
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
  description = "Decoded contents of `migrations/regional/generated/regional-tables.json`. The caller decodes the bundle; nothing in `infra/` reads a file."
}

variable "table_definitions_digest" {
  type        = string
  description = "Digest of the definition set actually passed in."
}

variable "expected_definitions_digest" {
  type        = string
  description = "Digest the release manifest pins for the regional table bundle."
}

variable "keystore_physical_name" {
  type        = string
  description = "Pinned, immutable physical name of the keystore table."
}

variable "content_bucket_suffix" {
  type        = string
  description = "Suffix that makes the content bucket name globally unique."
}

variable "content_lifecycle_role_arn" {
  type        = string
  description = "The only principal permitted to delete a content object."
}

variable "content_authority" {
  type        = string
  description = "Which authority key encrypts the content bucket."
  default     = "content"
}

variable "tags" {
  type        = map(string)
  description = "Tags applied to everything in the region foundation."
  default     = {}
}
