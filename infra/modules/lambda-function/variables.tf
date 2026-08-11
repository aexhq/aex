variable "function_name" {
  type        = string
  description = "Physical function name."

  validation {
    condition     = can(regex("^aex-[a-z0-9-]{3,50}$", var.function_name))
    error_message = "The function name must start with `aex-` and be lowercase and hyphen-separated."
  }
}

variable "artifact_bucket" {
  type        = string
  description = "Bucket holding the published ZIP. Terraform packages no code: the ZIP was built, digested and published by the release lane long before this plan runs."

  validation {
    condition     = can(regex("^[a-z0-9][a-z0-9.-]{2,62}$", var.artifact_bucket))
    error_message = "The artifact bucket must be a valid S3 bucket name."
  }
}

variable "artifact_key" {
  type        = string
  description = "Key of the published ZIP. The convention is `lambda/<unit>/<sha256>.zip`, so the key itself is immutable."

  validation {
    condition     = can(regex("^lambda/[a-z0-9-]+/[0-9a-f]{64}\\.zip$", var.artifact_key))
    error_message = "The artifact key must be `lambda/<unit>/<sha256-hex>.zip`."
  }
}

variable "artifact_object_version" {
  type        = string
  description = "S3 object version of the published ZIP. Pinning the version makes the deployed bytes unambiguous even if the key were ever rewritten."

  validation {
    condition     = length(var.artifact_object_version) > 0
    error_message = "An object version is required; an unversioned artifact reference is not deployable."
  }
}

variable "artifact_sha256" {
  type        = string
  description = "The S3 `ChecksumSHA256` of the published ZIP: base64 of the raw SHA-256 digest, which is the same digest the artifact envelope records in hex. The plan fails if the stored object does not match."

  validation {
    condition     = can(regex("^[A-Za-z0-9+/]{43}=$", var.artifact_sha256))
    error_message = "The checksum must be base64 of a raw SHA-256 digest, 44 characters ending in `=`."
  }
}

variable "architecture" {
  type        = string
  default     = "arm64"
  description = "Lambda architecture."

  validation {
    condition     = contains(["arm64", "x86_64"], var.architecture)
    error_message = "The architecture must be `arm64` or `x86_64`."
  }
}

variable "runtime" {
  type        = string
  default     = "provided.al2023"
  description = "Lambda runtime. Rust deployables use the custom runtime; the two TypeScript edge functions use a Node runtime."

  validation {
    condition     = contains(["provided.al2023", "nodejs22.x"], var.runtime)
    error_message = "The runtime must be `provided.al2023` or `nodejs22.x`."
  }
}

variable "handler" {
  type        = string
  default     = "bootstrap"
  description = "Handler symbol. The custom runtime always uses `bootstrap`."

  validation {
    condition     = length(var.handler) > 0
    error_message = "A handler is required."
  }
}

variable "memory_mb" {
  type        = number
  description = "Memory size in megabytes, which also fixes the CPU share."

  validation {
    condition     = var.memory_mb >= 128 && var.memory_mb <= 10240
    error_message = "Memory must be between 128 and 10240 megabytes."
  }
}

variable "timeout_s" {
  type        = number
  description = "Function timeout in seconds."

  validation {
    condition     = var.timeout_s >= 1 && var.timeout_s <= 900
    error_message = "The timeout must be between 1 and 900 seconds."
  }
}

variable "reserved_concurrency" {
  type        = number
  default     = -1
  description = "Reserved concurrency. `-1` means unreserved."

  validation {
    condition     = var.reserved_concurrency == -1 || var.reserved_concurrency >= 0
    error_message = "Reserved concurrency must be -1 or a non-negative number."
  }
}

variable "env" {
  type        = map(string)
  default     = {}
  description = "Environment variables. Every key is namespaced so a stray variable from another system can never be read as configuration."

  validation {
    condition     = alltrue([for k in keys(var.env) : can(regex("^AEX_[A-Z0-9_]+$", k))])
    error_message = "Every environment variable key must match `^AEX_[A-Z0-9_]+$`."
  }

  validation {
    condition     = alltrue([for v in values(var.env) : !can(regex("^(AKIA|ASIA|sk-|sk_live|pk_live|ghp_|github_pat_)", v))])
    error_message = "An environment value looks like a credential. Secrets are referenced by ARN through the binding, never inlined."
  }
}

variable "role_arn" {
  type        = string
  description = "Execution role, produced by `iam-deployable-role`."

  validation {
    condition     = can(regex("^arn:aws[a-z-]*:iam::[0-9A-Za-z-]{1,64}:role/[A-Za-z0-9+=,.@_/-]+$", var.role_arn))
    error_message = "The execution role must be an IAM role ARN."
  }
}

variable "log_retention_days" {
  type        = number
  description = "CloudWatch log group retention in days."

  validation {
    condition = contains(
      [1, 3, 5, 7, 14, 30, 60, 90, 120, 150, 180, 365, 400, 545, 731, 1096, 1827, 2192, 2557, 2922, 3288, 3653],
      var.log_retention_days
    )
    error_message = "The retention must be one of the CloudWatch retention values."
  }
}

variable "log_kms_key_arn" {
  type        = string
  default     = null
  description = "Optional customer-managed key for the log group."

  validation {
    condition     = var.log_kms_key_arn == null || can(regex("^arn:aws[a-z-]*:kms:", coalesce(var.log_kms_key_arn, "none")))
    error_message = "When set, the log key must be a KMS key ARN."
  }
}

variable "code_signing_config_arn" {
  type        = string
  default     = null
  description = "Optional code-signing configuration."

  validation {
    condition     = var.code_signing_config_arn == null || can(regex("^arn:aws[a-z-]*:lambda:", coalesce(var.code_signing_config_arn, "none")))
    error_message = "When set, the code-signing configuration must be a Lambda ARN."
  }
}

variable "alias_name" {
  type        = string
  default     = "live"
  description = "Alias every event source and every caller targets. Nothing ever invokes an unqualified function."

  validation {
    condition     = can(regex("^[a-z][a-z0-9-]{1,31}$", var.alias_name))
    error_message = "The alias name must be lowercase and hyphen-separated."
  }

  validation {
    condition     = var.alias_name != "$LATEST"
    error_message = "An alias may not be named `$LATEST`."
  }
}

variable "public_function_url_enabled" {
  type        = bool
  default     = false
  description = "Whether to expose the immutable alias through an unauthenticated, buffered HTTPS Function URL. The handler must authenticate the request at the application boundary."
}

variable "async_failure_destination_arn" {
  type        = string
  default     = null
  description = "Optional unconsumed, alarmed SQS DLQ ARN for failed asynchronous invocations. Setting it enables the alias-qualified async invocation policy."

  validation {
    condition     = var.async_failure_destination_arn == null || can(regex("^arn:aws[a-z-]*:sqs:[a-z0-9-]+:[0-9A-Za-z-]{1,64}:[A-Za-z0-9_-]+$", coalesce(var.async_failure_destination_arn, "none")))
    error_message = "When set, the asynchronous failure destination must be an SQS queue ARN."
  }
}

variable "async_max_event_age_seconds" {
  type        = number
  default     = 21600
  description = "Maximum age of an asynchronous event before it is sent to the failure destination."

  validation {
    condition     = var.async_max_event_age_seconds >= 60 && var.async_max_event_age_seconds <= 21600
    error_message = "Asynchronous event age must be between 60 seconds and 6 hours."
  }
}

variable "async_max_retry_attempts" {
  type        = number
  default     = 2
  description = "Function-error retries for asynchronous invocation. Lambda permits zero through two."

  validation {
    condition     = var.async_max_retry_attempts >= 0 && var.async_max_retry_attempts <= 2
    error_message = "Asynchronous retry attempts must be between zero and two."
  }
}

variable "tags" {
  type        = map(string)
  default     = {}
  description = "Tags applied to the function."
}
