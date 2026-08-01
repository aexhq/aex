variable "cluster_identifier" {
  type        = string
  description = "Cluster identifier."

  validation {
    condition     = can(regex("^aex-[a-z0-9-]{3,50}$", var.cluster_identifier))
    error_message = "The cluster identifier must start with `aex-` and be lowercase and hyphen-separated."
  }
}

variable "database_name" {
  type        = string
  description = "Initial database name."

  validation {
    condition     = can(regex("^[a-z][a-z0-9_]{0,62}$", var.database_name))
    error_message = "The database name must be lowercase snake_case."
  }
}

variable "master_username" {
  type        = string
  description = "Master user name. The password is managed by RDS and stored in Secrets Manager; it never appears in configuration or state."

  validation {
    condition     = can(regex("^[a-z][a-z0-9_]{2,15}$", var.master_username))
    error_message = "The master user name must be lowercase and 3-16 characters."
  }
}

variable "engine_version" {
  type        = string
  description = "Aurora PostgreSQL engine version."

  validation {
    condition     = can(regex("^[0-9]+\\.[0-9]+$", var.engine_version))
    error_message = "The engine version must be pinned as `<major>.<minor>`."
  }
}

variable "min_acu" {
  type        = number
  default     = 0.5
  description = "Minimum Aurora capacity units."

  validation {
    condition     = var.min_acu >= 0 && var.min_acu <= 256
    error_message = "The minimum capacity must be between 0 and 256 ACU."
  }
}

variable "max_acu" {
  type        = number
  default     = 8
  description = "Maximum Aurora capacity units."

  validation {
    condition     = var.max_acu >= var.min_acu && var.max_acu <= 256
    error_message = "The maximum capacity must be at least the minimum and at most 256 ACU."
  }
}

variable "backup_retention_days" {
  type        = number
  default     = 7
  description = "Automated backup retention in days. The floor is 7: this cluster holds the money ledger, and a shorter window would leave a weekend incident with nothing to restore from."

  validation {
    condition     = var.backup_retention_days >= 7 && var.backup_retention_days <= 35
    error_message = "Backup retention must be at least 7 days."
  }
}

variable "deletion_protection" {
  type        = bool
  default     = true
  description = "Deletion protection. It stays on."

  validation {
    condition     = var.deletion_protection
    error_message = "Deletion protection must stay enabled on the finance cluster."
  }
}

variable "data_api_enabled" {
  type        = bool
  default     = true
  description = "Whether the RDS Data API is enabled. It is the only transport the Lambda callers use, so it is always on."

  validation {
    condition     = var.data_api_enabled
    error_message = "The Data API must be enabled; it is the transport every caller uses."
  }
}

variable "publicly_accessible" {
  type        = bool
  default     = false
  description = "Whether the writer instance is publicly reachable. It never is."

  validation {
    condition     = var.publicly_accessible == false
    error_message = "The cluster must not be publicly accessible."
  }
}

variable "reader_count" {
  type        = number
  default     = 0
  description = "Reader instances. There are none at launch: a reader adds replica-lag semantics the finance code does not yet account for."

  validation {
    condition     = var.reader_count == 0
    error_message = "No reader instance at launch; add one only once replica lag is accounted for in the read paths."
  }
}

variable "subnet_ids" {
  type        = list(string)
  description = "Private subnets for the DB subnet group."

  validation {
    condition     = length(var.subnet_ids) >= 2
    error_message = "A DB subnet group needs at least two subnets in different availability zones."
  }
}

variable "vpc_security_group_ids" {
  type        = list(string)
  description = "Security groups attached to the cluster."

  validation {
    condition     = length(var.vpc_security_group_ids) > 0
    error_message = "At least one security group is required."
  }
}

variable "admin_secret_arn" {
  type        = string
  description = "Secrets Manager secret holding the admin credentials. Data API callers pass this ARN with every statement. The module never reads the secret value."

  validation {
    condition     = can(regex("^arn:aws[a-z-]*:secretsmanager:[a-z0-9-]+:[0-9A-Za-z-]{1,64}:secret:[A-Za-z0-9/_+=.@-]+$", var.admin_secret_arn))
    error_message = "The admin secret must be a Secrets Manager secret ARN."
  }
}

variable "region" {
  type        = string
  description = "AWS region. Used to check that the admin secret lives in the same region as the cluster."

  validation {
    condition     = can(regex("^[a-z]{2}-[a-z]+-[0-9]$", var.region))
    error_message = "The region must be an AWS region code such as `eu-west-1`."
  }
}

variable "kms_key_arn" {
  type        = string
  description = "Customer-managed key for storage and for the managed master-user secret."

  validation {
    condition     = can(regex("^arn:aws[a-z-]*:kms:[a-z0-9-]+:[0-9A-Za-z-]{1,64}:key/[0-9a-f-]+$", var.kms_key_arn))
    error_message = "A customer-managed KMS key ARN is required."
  }
}

variable "preferred_backup_window" {
  type        = string
  default     = "02:00-03:00"
  description = "Daily backup window in UTC."

  validation {
    condition     = can(regex("^[0-9]{2}:[0-9]{2}-[0-9]{2}:[0-9]{2}$", var.preferred_backup_window))
    error_message = "The backup window must be `hh:mm-hh:mm`."
  }
}

variable "tags" {
  type        = map(string)
  default     = {}
  description = "Tags applied to the cluster and its instances."
}
