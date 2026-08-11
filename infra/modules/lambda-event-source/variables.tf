variable "function_alias_arn" {
  type        = string
  description = "The alias the event source invokes. Never an unqualified function and never `$LATEST`: an event source that follows `$LATEST` silently changes what it runs the moment a deployment publishes."

  validation {
    condition     = can(regex("^arn:aws[a-z-]*:lambda:[a-z0-9-]+:[0-9A-Za-z-]{1,64}:function:[A-Za-z0-9_-]+:[A-Za-z0-9_-]+$", var.function_alias_arn))
    error_message = "The target must be a qualified Lambda alias ARN of the form `...:function:<name>:<alias>`."
  }

  validation {
    condition     = !endswith(var.function_alias_arn, ":$LATEST")
    error_message = "An event source may not target `$LATEST`."
  }
}

variable "source_arn" {
  type        = string
  description = "The SQS queue or DynamoDB stream the mapping reads."

  validation {
    condition = (
      can(regex("^arn:aws[a-z-]*:sqs:[a-z0-9-]+:[0-9A-Za-z-]{1,64}:[A-Za-z0-9_.-]+$", var.source_arn))
      || can(regex("^arn:aws[a-z-]*:dynamodb:[a-z0-9-]+:[0-9A-Za-z-]{1,64}:table/[A-Za-z0-9_.-]+/stream/.+$", var.source_arn))
    )
    error_message = "The source must be an SQS queue ARN or a DynamoDB stream ARN."
  }
}

variable "batch_size" {
  type        = number
  description = "Maximum records delivered per invocation."

  validation {
    condition     = var.batch_size >= 1 && var.batch_size <= 10000
    error_message = "The batch size must be between 1 and 10000."
  }
}

variable "max_batching_window" {
  type        = number
  default     = 0
  description = "Seconds the service waits to fill a batch before invoking."

  validation {
    condition     = var.max_batching_window >= 0 && var.max_batching_window <= 300
    error_message = "The batching window must be between 0 and 300 seconds."
  }
}

variable "scaling_config" {
  type = object({
    maximum_concurrency = number
  })
  default     = null
  description = "Queue-side concurrency ceiling. Only an SQS source supports it."

  validation {
    condition     = var.scaling_config == null || try(var.scaling_config.maximum_concurrency, 0) >= 2
    error_message = "The maximum concurrency must be at least 2."
  }

  validation {
    condition     = var.scaling_config == null || can(regex(":sqs:", var.source_arn))
    error_message = "A scaling configuration is only valid on an SQS event source."
  }
}

variable "partial_batch_response" {
  type        = bool
  default     = true
  description = "Whether the function reports per-record failures. It must: without it a single poison record redrives the whole batch."

  validation {
    condition     = var.partial_batch_response
    error_message = "Partial batch responses are mandatory; `ReportBatchItemFailures` must always be requested."
  }
}

variable "starting_position" {
  type        = string
  default     = null
  description = "Where a stream mapping starts reading. Required for a DynamoDB stream, forbidden for a queue."

  validation {
    condition     = var.starting_position == null || contains(["TRIM_HORIZON", "LATEST"], coalesce(var.starting_position, "TRIM_HORIZON"))
    error_message = "The starting position must be `TRIM_HORIZON` or `LATEST`."
  }

  validation {
    condition     = !can(regex(":dynamodb:", var.source_arn)) || var.starting_position != null
    error_message = "A DynamoDB stream mapping must declare a starting position."
  }

  validation {
    condition     = !can(regex(":sqs:", var.source_arn)) || var.starting_position == null
    error_message = "A queue mapping has no starting position."
  }
}

variable "enabled" {
  type        = bool
  default     = true
  description = "Whether the mapping is enabled."
}

variable "tags" {
  type        = map(string)
  default     = {}
  description = "Tags applied to the mapping."
}
