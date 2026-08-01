variable "dynamodb_local_image" {
  type        = string
  default     = "amazon/dynamodb-local:2.6.1"
  description = "DynamoDB Local image, pinned to the tag `release/policy/test-images.toml` records. It must match `compose.yaml` exactly."

  validation {
    condition     = var.dynamodb_local_image == "amazon/dynamodb-local:2.6.1"
    error_message = "The DynamoDB Local pin is fixed at `amazon/dynamodb-local:2.6.1`; change it in `release/policy/test-images.toml` first."
  }
}

variable "minio_image" {
  type        = string
  default     = "minio/minio:RELEASE.2025-04-22T22-12-26Z"
  description = "MinIO image, pinned to the tag `release/policy/test-images.toml` records. It must match `compose.yaml` exactly."

  validation {
    condition     = var.minio_image == "minio/minio:RELEASE.2025-04-22T22-12-26Z"
    error_message = "The MinIO pin is fixed at `minio/minio:RELEASE.2025-04-22T22-12-26Z`; change it in `release/policy/test-images.toml` first."
  }
}

variable "postgres_image" {
  type        = string
  default     = "postgres:17.5-bookworm"
  description = "PostgreSQL image, pinned to the tag `release/policy/test-images.toml` records. It must match `compose.yaml` exactly."

  validation {
    condition     = var.postgres_image == "postgres:17.5-bookworm"
    error_message = "The PostgreSQL pin is fixed at `postgres:17.5-bookworm`; change it in `release/policy/test-images.toml` first."
  }
}

variable "host" {
  type        = string
  default     = "127.0.0.1"
  description = "Address the compose services bind on. It is a loopback address; the local substrate is never reachable from another machine."

  validation {
    condition     = contains(["127.0.0.1", "localhost"], var.host)
    error_message = "The local substrate must bind to loopback only."
  }
}

variable "dynamodb_port" {
  type        = number
  default     = 8000
  description = "Host port DynamoDB Local listens on."

  validation {
    condition     = var.dynamodb_port > 1024 && var.dynamodb_port < 65536
    error_message = "The port must be an unprivileged port."
  }
}

variable "minio_port" {
  type        = number
  default     = 9000
  description = "Host port the MinIO S3 API listens on."

  validation {
    condition     = var.minio_port > 1024 && var.minio_port < 65536
    error_message = "The port must be an unprivileged port."
  }
}

variable "postgres_port" {
  type        = number
  default     = 5432
  description = "Host port PostgreSQL listens on."

  validation {
    condition     = var.postgres_port > 1024 && var.postgres_port < 65536
    error_message = "The port must be an unprivileged port."
  }
}

variable "postgres_database" {
  type        = string
  default     = "aex_local"
  description = "Database the local PostgreSQL creates on first start."

  validation {
    condition     = can(regex("^[a-z][a-z0-9_]{0,62}$", var.postgres_database))
    error_message = "The database name must be lowercase snake_case."
  }
}
