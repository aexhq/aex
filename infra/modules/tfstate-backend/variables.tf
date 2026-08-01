variable "plane" {
  type        = string
  description = "Deployment plane whose state this backend holds."

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

variable "kms_alias" {
  type        = string
  description = "Alias of the key that encrypts state. State can contain resource identifiers, so it is always encrypted with a customer-managed key."

  validation {
    condition     = can(regex("^alias/aex-[a-z0-9][a-z0-9-]*$", var.kms_alias))
    error_message = "The alias must look like `alias/aex-<name>`."
  }
}

variable "deletion_window_days" {
  type        = number
  default     = 30
  description = "Pending-deletion window for the state key."

  validation {
    condition     = var.deletion_window_days >= 30
    error_message = "The deletion window must be at least 30 days."
  }
}

variable "noncurrent_retention_days" {
  type        = number
  default     = 90
  description = "Days a superseded state version is kept."

  validation {
    condition     = var.noncurrent_retention_days >= 30
    error_message = "Superseded state versions must be kept for at least 30 days."
  }
}

variable "tags" {
  type        = map(string)
  default     = {}
  description = "Tags applied to the bucket and the key."
}
