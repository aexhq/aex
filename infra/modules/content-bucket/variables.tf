variable "plane" {
  type        = string
  description = "Deployment plane the bucket belongs to."

  validation {
    condition     = contains(["dev", "prd"], var.plane)
    error_message = "The plane must be `dev` or `prd`."
  }
}

variable "region" {
  type        = string
  description = "AWS region the bucket lives in. Content never leaves its region."

  validation {
    condition     = can(regex("^[a-z]{2}-[a-z]+-[0-9]$", var.region))
    error_message = "The region must be an AWS region code such as `eu-west-1`."
  }
}

variable "purpose" {
  type        = string
  description = "What this bucket holds: `content`, `observations`, and so on. It is the last component of the physical name, so one module serves every object store in a plane and no store carries another store's name. Supplied by the root; the module invents no account-derived value."

  validation {
    condition     = can(regex("^[a-z0-9][a-z0-9-]{3,20}$", var.purpose))
    error_message = "The purpose must be 4-21 lowercase alphanumeric or hyphen characters."
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

variable "kms_key_arn" {
  type        = string
  description = "Customer-managed key that encrypts every object. The bucket policy denies a write encrypted with any other key."

  validation {
    condition     = can(regex("^arn:aws[a-z-]*:kms:[a-z0-9-]+:[0-9A-Za-z-]{1,64}:key/[0-9a-f-]+$", var.kms_key_arn))
    error_message = "A customer-managed KMS key ARN is required."
  }
}

variable "lifecycle_role_arn" {
  type        = string
  default     = null
  nullable    = true
  description = "Optional sole role permitted to delete an object. When null, bucket policy denies deletion to every principal."

  validation {
    condition     = var.lifecycle_role_arn == null || can(regex("^arn:aws[a-z-]*:iam::[0-9A-Za-z-]{1,64}:role/[A-Za-z0-9+=,.@_/-]+$", var.lifecycle_role_arn))
    error_message = "When supplied, the lifecycle role must be an IAM role ARN."
  }
}

variable "signature_age_ms" {
  type        = number
  default     = 300000
  description = "Maximum permitted request signature age in milliseconds. The bucket policy denies anything older, which caps how long a leaked presigned URL stays usable."

  validation {
    condition     = var.signature_age_ms > 0 && var.signature_age_ms <= 300000
    error_message = "The signature age ceiling must be greater than 0 and at most 300000 milliseconds."
  }
}

variable "browser_cors_origins" {
  type        = set(string)
  default     = []
  description = "Exact HTTPS browser origins allowed to use presigned downloads and multipart part uploads. An empty set keeps CORS disabled."

  validation {
    condition = alltrue([
      for origin in var.browser_cors_origins :
      can(regex("^https://[a-z0-9]([a-z0-9.-]*[a-z0-9])?(:[0-9]{1,5})?$", origin))
    ])
    error_message = "Every browser CORS origin must be a bare HTTPS origin without a path or trailing slash."
  }
}

variable "abort_incomplete_multipart_days" {
  type        = number
  default     = 1
  description = "Days after which an incomplete multipart upload is aborted."

  validation {
    condition     = var.abort_incomplete_multipart_days == 1
    error_message = "Incomplete multipart uploads must be aborted after 24 hours."
  }
}

variable "tags" {
  type        = map(string)
  default     = {}
  description = "Tags applied to the bucket."
}
