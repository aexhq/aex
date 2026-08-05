variable "region" {
  type        = string
  description = "AWS region containing the dedicated model-catalog signing key."

  validation {
    condition     = can(regex("^[a-z]{2}-[a-z]+-[0-9]$", var.region))
    error_message = "region must be an AWS region code."
  }
}

variable "account_id" {
  type        = string
  description = "Exact AWS account allowed to own the key and publisher role."

  validation {
    condition     = can(regex("^[0-9]{12}$", var.account_id))
    error_message = "account_id must be twelve digits."
  }
}

variable "oidc_provider_arn" {
  type        = string
  description = "Existing account-level GitHub Actions OIDC provider ARN."

  validation {
    condition     = can(regex("^arn:aws[a-z-]*:iam::${var.account_id}:oidc-provider/token\\.actions\\.githubusercontent\\.com$", var.oidc_provider_arn))
    error_message = "oidc_provider_arn must be the GitHub Actions provider in account_id."
  }
}

variable "kms_administrator_principal_arns" {
  type        = list(string)
  description = "Exact owner-controlled IAM principals allowed to administer the dedicated key."

  validation {
    condition = length(var.kms_administrator_principal_arns) > 0 && alltrue([
      for arn in var.kms_administrator_principal_arns : can(regex("^arn:aws[a-z-]*:iam::${var.account_id}:(?:role/[A-Za-z0-9+=,.@_/-]+|user/[A-Za-z0-9+=,.@_/-]+)$", arn))
    ])
    error_message = "At least one exact administrator role or user ARN in account_id is required."
  }
}

variable "repository" {
  type        = string
  description = "Exact public GitHub repository allowed to publish catalogs."
  default     = "aexhq/aex"

  validation {
    condition     = var.repository == "aexhq/aex"
    error_message = "Only the public aexhq/aex repository owns model-catalog publication."
  }
}

variable "environment" {
  type        = string
  description = "Exact protected GitHub Environment required by the publisher job."
  default     = "aex-model-catalog-publisher"

  validation {
    condition     = var.environment == "aex-model-catalog-publisher"
    error_message = "The publisher must use the dedicated aex-model-catalog-publisher environment."
  }
}

variable "workflow_name" {
  type        = string
  description = "Exact GitHub OIDC workflow claim allowed to assume the role."
  default     = "model catalog publish"

  validation {
    condition     = var.workflow_name == "model catalog publish"
    error_message = "The OIDC workflow claim must name the reviewed publisher workflow."
  }
}

variable "logical_key_id" {
  type        = string
  description = "Stable public keyId emitted in the model-catalog trust-root document."

  validation {
    condition     = can(regex("^[A-Za-z0-9_-]{1,64}$", var.logical_key_id))
    error_message = "logical_key_id must be 1-64 ASCII alphanumeric, hyphen, or underscore characters."
  }
}

variable "tags" {
  type        = map(string)
  default     = {}
  description = "Additional tags."
}
