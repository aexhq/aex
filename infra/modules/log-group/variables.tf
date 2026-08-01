variable "name" {
  type        = string
  description = "Log group name. It is the one namespace in this architecture expressed as a path rather than a prefix, and the plane is inside it: `/aex/<plane>/<deployable>`."

  validation {
    condition     = can(regex("^/aex/(dev|prd)/[a-z0-9][a-z0-9./_-]*$", var.name))
    error_message = "A log group must be named `/aex/<plane>/<deployable>`. A group outside that namespace is a group an IAM policy scoped to the plane cannot reach, so a deployable would silently write nowhere."
  }
}

variable "retention_days" {
  type        = number
  description = "Retention in days. Never unset: a group with no retention keeps every line forever and bills for it."

  validation {
    condition = contains(
      [1, 3, 5, 7, 14, 30, 60, 90, 120, 150, 180, 365, 400, 545, 731, 1096, 1827, 2192, 2557, 2922, 3288, 3653],
      var.retention_days
    )
    error_message = "The retention must be one of the retention windows CloudWatch Logs accepts. `0`, which means never expire, is not one of them here."
  }
}

variable "kms_key_arn" {
  type        = string
  description = "Customer-managed key that encrypts the log group. Logs carry request identifiers, workspace ids and error detail, so encryption is mandatory and there is no null case."

  validation {
    condition     = can(regex("^arn:aws[a-z-]*:kms:[a-z0-9-]+:[0-9A-Za-z-]{1,64}:key/[0-9a-f-]+$", var.kms_key_arn))
    error_message = "A customer-managed KMS key ARN is required; the CloudWatch Logs default key is not accepted."
  }
}

variable "tags" {
  type        = map(string)
  default     = {}
  description = "Tags applied to the log group."
}
