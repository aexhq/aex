variable "name" {
  type        = string
  description = "Pipe name."

  validation {
    condition     = can(regex("^aex-[a-z0-9-]{3,50}$", var.name))
    error_message = "The pipe name must start with `aex-` and be lowercase and hyphen-separated."
  }
}

variable "source_stream_arn" {
  type        = string
  description = "DynamoDB stream the pipe reads."

  validation {
    condition     = can(regex("^arn:aws[a-z-]*:dynamodb:[a-z0-9-]+:[0-9A-Za-z-]{1,64}:table/[A-Za-z0-9_.-]+/stream/.+$", var.source_stream_arn))
    error_message = "The source must be a DynamoDB stream ARN."
  }
}

variable "target_queue_arn" {
  type        = string
  description = "SQS queue the pipe writes to. The pipe role is granted `sqs:SendMessage` on this queue and on nothing else."

  validation {
    condition     = can(regex("^arn:aws[a-z-]*:sqs:[a-z0-9-]+:[0-9A-Za-z-]{1,64}:[A-Za-z0-9_.-]+$", var.target_queue_arn))
    error_message = "The target must be an SQS queue ARN."
  }
}

variable "filter_pattern" {
  type        = string
  description = "EventBridge filter pattern as a JSON document. An unfiltered pipe forwards every record and turns a table into a firehose, so a pattern is mandatory."

  validation {
    condition     = length(trimspace(var.filter_pattern)) > 0
    error_message = "A filter pattern is mandatory; an unfiltered pipe is not an allowed configuration."
  }

  validation {
    condition     = can(jsondecode(var.filter_pattern))
    error_message = "The filter pattern must parse as JSON."
  }

  validation {
    condition     = can(keys(jsondecode(var.filter_pattern))) && length(keys(jsondecode(var.filter_pattern))) > 0
    error_message = "The filter pattern must be a non-empty JSON object."
  }
}

variable "batch_size" {
  type        = number
  default     = 10
  description = "Records per batch delivered to the target."

  validation {
    condition     = var.batch_size >= 1 && var.batch_size <= 10000
    error_message = "The batch size must be between 1 and 10000."
  }
}

variable "starting_position" {
  type        = string
  default     = "LATEST"
  description = "Where the pipe starts reading the stream."

  validation {
    condition     = contains(["TRIM_HORIZON", "LATEST"], var.starting_position)
    error_message = "The starting position must be `TRIM_HORIZON` or `LATEST`."
  }
}

variable "maximum_batching_window" {
  type        = number
  default     = 1
  description = "Seconds the pipe waits to fill a batch."

  validation {
    condition     = var.maximum_batching_window >= 0 && var.maximum_batching_window <= 300
    error_message = "The batching window must be between 0 and 300 seconds."
  }
}

variable "stream_kms_key_arn" {
  type        = string
  default     = null
  description = "Key the source table is encrypted with, when the pipe role needs decrypt on it."

  validation {
    condition     = var.stream_kms_key_arn == null || can(regex("^arn:aws[a-z-]*:kms:", coalesce(var.stream_kms_key_arn, "none")))
    error_message = "When set, the stream key must be a KMS key ARN."
  }
}

variable "target_kms_key_arn" {
  type        = string
  default     = null
  description = "Key the target queue is encrypted with, when the pipe role needs encrypt on it."

  validation {
    condition     = var.target_kms_key_arn == null || can(regex("^arn:aws[a-z-]*:kms:", coalesce(var.target_kms_key_arn, "none")))
    error_message = "When set, the target key must be a KMS key ARN."
  }
}

variable "tags" {
  type        = map(string)
  default     = {}
  description = "Tags applied to the pipe and its role."
}
