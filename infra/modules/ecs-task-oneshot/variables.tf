variable "family" {
  type        = string
  description = "Task definition family. One family per one-shot task."

  validation {
    condition     = can(regex("^aex-[a-z0-9-]{3,50}$", var.family))
    error_message = "The family must start with `aex-` and be lowercase and hyphen-separated."
  }
}

variable "image" {
  type        = string
  description = "Container image, pinned by digest. A tag is rejected: the bytes a one-shot task runs are part of a release identity, and a tag can be moved."

  validation {
    condition     = can(regex("^[^:]+@sha256:[0-9a-f]{64}$", var.image))
    error_message = "The image must be digest-pinned as `<repository>@sha256:<64 hex>`; a `:tag` reference is not accepted."
  }
}

variable "cpu" {
  type        = number
  description = "Task CPU units."

  validation {
    condition     = contains([256, 512, 1024, 2048, 4096, 8192, 16384], var.cpu)
    error_message = "The CPU value must be a valid Fargate task size."
  }
}

variable "memory" {
  type        = number
  description = "Task memory in megabytes."

  validation {
    condition     = var.memory >= 512 && var.memory <= 122880
    error_message = "Memory must be between 512 and 122880 megabytes."
  }
}

variable "role_arn" {
  type        = string
  description = "Task role the container runs as."

  validation {
    condition     = can(regex("^arn:aws[a-z-]*:iam::[0-9A-Za-z-]{1,64}:role/[A-Za-z0-9+=,.@_/-]+$", var.role_arn))
    error_message = "The task role must be an IAM role ARN."
  }
}

variable "execution_role_arn" {
  type        = string
  description = "Execution role the ECS agent uses to pull the image and write logs."

  validation {
    condition     = can(regex("^arn:aws[a-z-]*:iam::[0-9A-Za-z-]{1,64}:role/[A-Za-z0-9+=,.@_/-]+$", var.execution_role_arn))
    error_message = "The execution role must be an IAM role ARN."
  }
}

variable "subnets" {
  type        = list(string)
  description = "Private subnets the task runs in. The caller passes these straight through to `RunTask`."

  validation {
    condition     = length(var.subnets) > 0
    error_message = "At least one subnet is required."
  }

  validation {
    condition     = alltrue([for s in var.subnets : can(regex("^subnet-[0-9a-f]{8,32}$", s))])
    error_message = "Every subnet must be a subnet id."
  }
}

variable "assign_public_ip" {
  type        = bool
  default     = false
  description = "Whether the task gets a public address. It never does: a one-shot task reaches AWS through interface endpoints, not through the internet."

  validation {
    condition     = var.assign_public_ip == false
    error_message = "A one-shot task must not be given a public IP."
  }
}

variable "runtime_platform" {
  type = object({
    cpu_architecture        = string
    operating_system_family = string
  })
  default = {
    cpu_architecture        = "ARM64"
    operating_system_family = "LINUX"
  }
  description = "Explicit runtime platform. It is declared rather than inferred so an image index cannot silently select a different child."

  validation {
    condition     = contains(["ARM64", "X86_64"], var.runtime_platform.cpu_architecture)
    error_message = "The CPU architecture must be `ARM64` or `X86_64`."
  }
}

variable "log_group_name" {
  type        = string
  description = "CloudWatch log group the container writes to."

  validation {
    condition     = startswith(var.log_group_name, "/aex/")
    error_message = "The log group must live under `/aex/`."
  }
}

variable "region" {
  type        = string
  description = "AWS region, used for the log driver configuration."

  validation {
    condition     = can(regex("^[a-z]{2}-[a-z]+-[0-9]$", var.region))
    error_message = "The region must be an AWS region code such as `eu-west-1`."
  }
}

variable "env" {
  type        = map(string)
  default     = {}
  description = "Environment variables. Every key is namespaced."

  validation {
    condition     = alltrue([for k in keys(var.env) : can(regex("^AEX_[A-Z0-9_]+$", k))])
    error_message = "Every environment variable key must match `^AEX_[A-Z0-9_]+$`."
  }
}

variable "secret_env" {
  type        = map(string)
  default     = {}
  description = "Environment variables sourced from Secrets Manager or SSM by ARN. Values are references, never plaintext."

  validation {
    condition     = alltrue([for k in keys(var.secret_env) : can(regex("^AEX_[A-Z0-9_]+$", k))])
    error_message = "Every secret environment key must match `^AEX_[A-Z0-9_]+$`."
  }

  validation {
    condition     = alltrue([for v in values(var.secret_env) : startswith(v, "arn:")])
    error_message = "Every secret value must be an ARN reference; a plaintext secret is not accepted."
  }
}

variable "tags" {
  type        = map(string)
  default     = {}
  description = "Tags applied to the task definition."
}
