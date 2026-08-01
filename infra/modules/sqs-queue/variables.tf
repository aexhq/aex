variable "name" {
  type        = string
  description = "Base queue name without the `.fifo` suffix. The module appends the suffix when `fifo` is set."

  validation {
    condition     = can(regex("^[a-z][a-z0-9-]{2,70}$", var.name))
    error_message = "The queue name must be lowercase, hyphen-separated and 3-71 characters."
  }
}

variable "fifo" {
  type        = bool
  default     = false
  description = "Whether the queue is FIFO. The usage-rating queues must be FIFO because rating is order-sensitive per workspace."

  validation {
    condition     = !startswith(var.name, "usage-rating") || var.fifo
    error_message = "A usage-rating queue must be FIFO."
  }
}

variable "content_based_dedup" {
  type        = bool
  default     = false
  description = "Whether FIFO deduplication is derived from the message body instead of an explicit deduplication id."

  validation {
    condition     = !var.content_based_dedup || var.fifo
    error_message = "Content-based deduplication only exists on a FIFO queue."
  }
}

variable "visibility_timeout" {
  type        = number
  description = "Visibility timeout in seconds. Set it to at least six times the consumer's worst-case handling time."

  validation {
    condition     = var.visibility_timeout >= 1 && var.visibility_timeout <= 43200
    error_message = "The visibility timeout must be between 1 and 43200 seconds."
  }
}

variable "max_receive_count" {
  type        = number
  description = "Number of deliveries after which a message is redriven to the dead-letter queue."

  validation {
    condition     = var.max_receive_count >= 1 && var.max_receive_count <= 1000
    error_message = "The maximum receive count must be between 1 and 1000."
  }
}

variable "message_retention_seconds" {
  type        = number
  default     = 345600
  description = "Retention of the main queue in seconds."

  validation {
    condition     = var.message_retention_seconds >= 60 && var.message_retention_seconds <= 1209600
    error_message = "Message retention must be between 60 and 1209600 seconds."
  }
}

variable "kms_key_arn" {
  type        = string
  description = "Customer-managed KMS key that encrypts messages at rest. Server-side encryption is mandatory, so there is no null case."

  validation {
    condition     = can(regex("^arn:aws[a-z-]*:kms:[a-z0-9-]+:[0-9A-Za-z-]{1,64}:key/[0-9a-f-]+$", var.kms_key_arn))
    error_message = "A customer-managed KMS key ARN is required; SQS-managed encryption is not accepted."
  }
}

variable "kms_data_key_reuse_period_seconds" {
  type        = number
  default     = 300
  description = "How long SQS may reuse a data key before calling KMS again."

  validation {
    condition     = var.kms_data_key_reuse_period_seconds >= 60 && var.kms_data_key_reuse_period_seconds <= 86400
    error_message = "The data key reuse period must be between 60 and 86400 seconds."
  }
}

variable "dlq" {
  type = object({
    enabled                   = bool
    message_retention_seconds = number
  })
  description = "Dead-letter queue settings. A dead-letter queue is mandatory: `enabled = false` is rejected rather than silently skipped."

  validation {
    condition     = var.dlq.enabled
    error_message = "A dead-letter queue is mandatory for every aex queue."
  }

  validation {
    condition     = var.dlq.message_retention_seconds >= 86400 && var.dlq.message_retention_seconds <= 1209600
    error_message = "Dead-letter retention must be between 86400 and 1209600 seconds so a failure survives a weekend."
  }
}

variable "policy_statements" {
  type = list(object({
    sid            = string
    effect         = string
    principal_type = string
    principals     = list(string)
    actions        = list(string)
  }))
  default     = []
  description = "Resource policy statements. The module scopes every statement to this queue; the root supplies the principals and actions."

  validation {
    condition     = alltrue([for s in var.policy_statements : contains(["Allow", "Deny"], s.effect)])
    error_message = "Every statement effect must be `Allow` or `Deny`."
  }

  validation {
    condition     = alltrue([for s in var.policy_statements : contains(["AWS", "Service"], s.principal_type)])
    error_message = "Every statement principal type must be `AWS` or `Service`."
  }

  validation {
    condition     = alltrue([for s in var.policy_statements : length(s.principals) > 0 && !contains(s.principals, "*")])
    error_message = "A wildcard principal is not allowed in a queue policy; name every principal explicitly."
  }

  validation {
    condition     = alltrue([for s in var.policy_statements : !contains(s.actions, "*") && !contains(s.actions, "sqs:*")])
    error_message = "A wildcard action is not allowed in a queue policy; enumerate the SQS actions."
  }
}

variable "tags" {
  type        = map(string)
  default     = {}
  description = "Tags applied to both queues."
}
