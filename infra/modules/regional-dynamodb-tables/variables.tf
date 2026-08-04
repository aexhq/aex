variable "plane" {
  type        = string
  description = "Deployment plane this regional store belongs to."

  validation {
    condition     = contains(["dev", "prd"], var.plane)
    error_message = "The plane must be `dev` or `prd`."
  }
}

variable "region" {
  type        = string
  description = "AWS region the tables live in. Regional stores never span regions."

  validation {
    condition     = can(regex("^[a-z]{2}-[a-z]+-[0-9]$", var.region))
    error_message = "The region must be an AWS region code such as `eu-west-1`."
  }
}

variable "name_prefix" {
  type        = string
  description = "Physical name prefix applied to every table that does not declare a pinned physical name."

  validation {
    condition     = can(regex("^aex-[a-z0-9-]+-$", var.name_prefix))
    error_message = "The name prefix must start with `aex-` and end with a hyphen."
  }
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
  description = "The already-decoded contents of `migrations/regional/generated/regional-tables.json`. The module never reads a file; the root decodes the bundle and passes the result in."

  validation {
    condition     = length(var.table_definitions) > 0
    error_message = "At least one table definition is required."
  }

  validation {
    condition     = length(distinct([for t in var.table_definitions : t.logical_name])) == length(var.table_definitions)
    error_message = "Logical table names must be unique."
  }

  validation {
    condition     = alltrue([for t in var.table_definitions : can(regex("^[a-z][a-z0-9_]*$", t.logical_name))])
    error_message = "Logical table names must be lowercase snake_case."
  }

  validation {
    condition     = alltrue([for t in var.table_definitions : t.billing_mode == "PAY_PER_REQUEST"])
    error_message = "Every regional table must be PAY_PER_REQUEST; provisioned capacity is not an allowed configuration."
  }

  validation {
    condition     = alltrue([for t in var.table_definitions : t.point_in_time_recovery_days >= 35])
    error_message = "Point-in-time recovery must retain at least 35 days on every table."
  }

  validation {
    condition     = alltrue([for t in var.table_definitions : t.deletion_protection])
    error_message = "Deletion protection must be on for every regional table."
  }

  validation {
    condition = alltrue([
      for t in var.table_definitions :
      t.ttl_attribute == null || t.ttl_attribute == "expiresAtEpochSeconds"
    ])
    error_message = "The only permitted TTL attribute name is `expiresAtEpochSeconds`."
  }

  validation {
    condition = alltrue(flatten([
      for t in var.table_definitions : [
        for g in coalesce(t.global_secondary_indexes, []) : g.projection_type != "ALL"
      ]
    ]))
    error_message = "A global secondary index may not use projection type `ALL`; project `KEYS_ONLY` or an explicit attribute list."
  }

  validation {
    condition = alltrue(flatten([
      for t in var.table_definitions : [
        for g in coalesce(t.global_secondary_indexes, []) :
        contains(["KEYS_ONLY", "INCLUDE"], g.projection_type)
      ]
    ]))
    error_message = "A global secondary index projection type must be `KEYS_ONLY` or `INCLUDE`."
  }

  validation {
    condition = alltrue(flatten([
      for t in var.table_definitions : [
        for g in coalesce(t.global_secondary_indexes, []) :
        g.projection_type != "INCLUDE" || length(coalesce(g.non_key_attributes, [])) > 0
      ]
    ]))
    error_message = "An `INCLUDE` projection must name the attributes it projects; an empty list is `KEYS_ONLY` written ambiguously."
  }

  validation {
    condition = alltrue(flatten([
      for t in var.table_definitions : [
        for g in coalesce(t.global_secondary_indexes, []) :
        g.projection_type != "KEYS_ONLY" || length(coalesce(g.non_key_attributes, [])) == 0
      ]
    ]))
    error_message = "A `KEYS_ONLY` projection may not name projected attributes; AWS rejects the index and the intent is contradictory."
  }

  validation {
    condition = alltrue(flatten([
      for t in var.table_definitions : [
        for g in coalesce(t.global_secondary_indexes, []) :
        g.projection_type != "INCLUDE" || length(coalesce(g.non_key_attributes, [])) <= 20
      ]
    ]))
    error_message = "An `INCLUDE` projection may name at most 20 non-key attributes; this is a DynamoDB hard limit."
  }

  validation {
    condition = alltrue([
      for t in var.table_definitions : sum(concat([0], [
        for g in coalesce(t.global_secondary_indexes, []) :
        g.projection_type == "INCLUDE" ? length(coalesce(g.non_key_attributes, [])) : 0
      ])) <= 100
    ])
    error_message = "The non-key attributes projected by all `INCLUDE` indexes on one table may sum to at most 100; repeated names count once per index."
  }

  validation {
    condition = alltrue([
      for t in var.table_definitions :
      t.pinned_physical_name == null || can(regex("^[a-zA-Z0-9_.-]{3,255}$", coalesce(t.pinned_physical_name, "none")))
    ])
    error_message = "A pinned physical name must be a valid DynamoDB table name."
  }

  validation {
    condition = alltrue([
      for t in var.table_definitions :
      t.pinned_physical_name == null || !startswith(coalesce(t.pinned_physical_name, "none"), var.name_prefix)
    ])
    error_message = "A pinned name is pinned because it is bound to the table's own contents; one derived from `name_prefix` is a derived name pretending to be pinned."
  }

  validation {
    condition = alltrue(flatten([
      for t in var.table_definitions : [for a in t.attributes : contains(["S", "N", "B"], a.type)]
    ]))
    error_message = "Attribute types must be `S`, `N` or `B`."
  }

  validation {
    condition = alltrue([
      for t in var.table_definitions :
      t.stream_view_type == null || contains(["NEW_IMAGE", "OLD_IMAGE", "NEW_AND_OLD_IMAGES", "KEYS_ONLY"], coalesce(t.stream_view_type, "NEW_IMAGE"))
    ])
    error_message = "An enabled stream must name a valid view type."
  }
}

variable "kms_key_arn_by_authority" {
  type        = map(string)
  description = "Customer-managed key ARN per authority. Every table names an authority and is encrypted with that authority's key; there is no AWS-managed fallback."

  validation {
    condition = alltrue([
      for k, v in var.kms_key_arn_by_authority :
      can(regex("^arn:aws[a-z-]*:kms:[a-z0-9-]+:[0-9A-Za-z-]{1,64}:key/[0-9a-f-]+$", v))
    ])
    error_message = "Every authority must map to a customer-managed KMS key ARN."
  }

  validation {
    condition     = length(var.kms_key_arn_by_authority) > 0
    error_message = "At least one authority key is required."
  }

  validation {
    condition = alltrue([
      for t in var.table_definitions : contains(keys(var.kms_key_arn_by_authority), t.authority)
    ])
    error_message = "Every table authority must have a customer-managed key; there is no default key."
  }
}

variable "table_definitions_digest" {
  type        = string
  description = "The digest the decoded bundle carries for its own definition set. It is `blake3:` because that is the digest the bundler computes over the rendered table array and the only digest that exists for this artefact; a second digest over the same bytes would be a drift surface, not a second proof."

  validation {
    condition     = can(regex("^blake3:[0-9a-f]{64}$", var.table_definitions_digest))
    error_message = "The definitions digest must be `blake3:` followed by 64 hex characters, exactly as `migrations/regional/generated/regional-tables.json` carries it."
  }

  validation {
    condition     = var.table_definitions_digest == var.expected_definitions_digest
    error_message = "The decoded definition digest does not match the digest the release manifest pins; the root is planning against a different bundle."
  }
}

variable "expected_definitions_digest" {
  type        = string
  description = "Digest the release manifest pins for `migrations/regional/generated/regional-tables.json`. It is the same value the bundle carries, in the same domain."

  validation {
    condition     = can(regex("^blake3:[0-9a-f]{64}$", var.expected_definitions_digest))
    error_message = "The expected definitions digest must be `blake3:` followed by 64 hex characters."
  }
}

variable "tags" {
  type        = map(string)
  default     = {}
  description = "Tags applied to every table."
}
