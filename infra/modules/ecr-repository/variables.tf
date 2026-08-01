variable "name" {
  type        = string
  description = "Repository name."

  validation {
    condition     = can(regex("^aex/[a-z0-9][a-z0-9._/-]{2,200}$", var.name))
    error_message = "The repository name must live under the `aex/` namespace."
  }
}

variable "immutable_tags" {
  type        = bool
  default     = true
  description = "Tag immutability. It stays on: a mutable tag would let the same reference point at different bytes on different days, which is exactly what the release manifest exists to prevent."

  validation {
    condition     = var.immutable_tags
    error_message = "Tag immutability must stay enabled."
  }
}

variable "scan_on_push" {
  type        = bool
  default     = true
  description = "Whether a pushed image is scanned immediately."
}

variable "lifecycle_by_reference" {
  type = object({
    untagged_expire_days = number
  })
  description = "Expiry policy. Only untagged, therefore unreferenced, digests are ever expired: a tagged image may still be named by a released manifest."

  validation {
    condition     = var.lifecycle_by_reference.untagged_expire_days >= 1 && var.lifecycle_by_reference.untagged_expire_days <= 365
    error_message = "The untagged expiry must be between 1 and 365 days."
  }
}

variable "kms_key_arn" {
  type        = string
  default     = null
  description = "Optional customer-managed key for repository encryption. AES256 when null."

  validation {
    condition     = var.kms_key_arn == null || can(regex("^arn:aws[a-z-]*:kms:", coalesce(var.kms_key_arn, "none")))
    error_message = "When set, the key must be a KMS key ARN."
  }
}

variable "force_delete" {
  type        = bool
  default     = false
  description = "Whether the repository may be deleted while it still holds images."

  validation {
    condition     = var.force_delete == false
    error_message = "A repository holding released image bytes must not be force-deletable."
  }
}

variable "tags" {
  type        = map(string)
  default     = {}
  description = "Tags applied to the repository."
}
