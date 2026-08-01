variable "plane" {
  type        = string
  description = "Deployment plane the bucket belongs to."

  validation {
    condition     = contains(["dev", "prd"], var.plane)
    error_message = "The plane must be `dev` or `prd`."
  }
}

variable "bucket_name_suffix" {
  type        = string
  description = "Suffix that makes the bucket name globally unique. Supplied by the root."

  validation {
    condition     = can(regex("^[a-z0-9][a-z0-9-]{3,20}$", var.bucket_name_suffix))
    error_message = "The suffix must be 4-21 lowercase alphanumeric or hyphen characters."
  }
}

variable "partition" {
  type        = string
  default     = "aws"
  description = "AWS partition used to construct the bucket ARN."

  validation {
    condition     = can(regex("^aws[a-z-]*$", var.partition))
    error_message = "The partition must be `aws` or an `aws-` prefixed partition name."
  }
}

variable "retention_days" {
  type        = number
  description = "Days a noncurrent version is kept before it expires."

  validation {
    condition     = var.retention_days >= 1 && var.retention_days <= 3650
    error_message = "Noncurrent retention must be between 1 and 3650 days."
  }
}

variable "kms_key_arn" {
  type        = string
  default     = null
  description = "Customer-managed key. When null the bucket uses SSE-S3, which is acceptable here because the bucket holds small infrastructure artifacts and no customer data."

  validation {
    condition     = var.kms_key_arn == null || can(regex("^arn:aws[a-z-]*:kms:[a-z0-9-]+:[0-9A-Za-z-]{1,64}:key/[0-9a-f-]+$", coalesce(var.kms_key_arn, "none")))
    error_message = "When set, the key must be a KMS key ARN."
  }
}

variable "object_lock_enabled" {
  type        = bool
  default     = false
  description = "Object Lock. It stays off: this bucket holds infrastructure artifacts whose identity is their digest, and a legal hold would make cleanup impossible."

  validation {
    condition     = var.object_lock_enabled == false
    error_message = "Object Lock must stay off on the artifact bucket."
  }
}

variable "tags" {
  type        = map(string)
  default     = {}
  description = "Tags applied to the bucket."
}
