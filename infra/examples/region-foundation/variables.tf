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
    allow_nat          = bool
    nat_justification  = string
  })
  description = "Regional network shape. NAT is explicit because provider and MCP calls leave AWS from private Brain/Tool tasks."
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
      conditions = optional(list(object({
        test     = string
        variable = string
        values   = list(string)
      })), [])
    }))
  }))
  description = "One customer-managed key per authority. Table encryption is resolved from a table's authority, so every authority a table names must appear here."
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
  description = "Decoded contents of `migrations/regional/generated/regional-tables.json`. The caller decodes the bundle; nothing in `infra/` reads a file."
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
