variable "plane" {
  type        = string
  description = "Deployment plane this account backs."
}

variable "region" {
  type        = string
  description = "Region the ops topic and budgets live in."
}

variable "account_id" {
  type        = string
  description = "Account the backbone is created in. Supplied by the caller; nothing here discovers it."
}

variable "state_bucket_suffix" {
  type        = string
  description = "Suffix that makes the state bucket name globally unique."
}

variable "state_kms_alias" {
  type        = string
  description = "Alias of the key that encrypts Terraform state."
}

variable "artifact_bucket_suffix" {
  type        = string
  description = "Suffix that makes the artifact bucket name globally unique."
}

variable "artifact_retention_days" {
  type        = number
  description = "Days a superseded infrastructure artifact version is kept."
}

variable "ops_topic_name" {
  type        = string
  description = "Name of the operational notification topic."
}

variable "ops_publish_principals" {
  type = list(object({
    type       = string
    identifier = string
  }))
  description = "Explicit publishers on the ops topic."
}

variable "ops_subscriptions" {
  type = list(object({
    protocol = string
    endpoint = string
  }))
  description = "Subscriptions on the ops topic."
}

variable "github_role_name" {
  type        = string
  description = "Exact plane-scoped name of the GitHub publish role."
}

variable "github_repository" {
  type        = string
  description = "The one repository whose workflows may assume the publish role."
}

variable "github_oidc_provider_arn" {
  type        = string
  description = "ARN of the account's GitHub OIDC provider."
}

variable "github_allowed_refs" {
  type        = list(string)
  description = "Exact refs permitted to assume the publish role."
}

variable "github_allowed_workflows" {
  type        = list(string)
  description = "Exact workflow paths permitted to assume the publish role."
}

variable "permission_profiles" {
  type        = map(list(string))
  description = "Actions granted by each of the four permission profiles."
}

variable "profile_resources" {
  type        = map(list(string))
  description = "Resources each profile's actions apply to."
}

variable "budgets" {
  type = list(object({
    name              = string
    limit_amount      = number
    limit_unit        = string
    time_unit         = string
    threshold_percent = number
    cost_filter_tags  = map(list(string))
  }))
  description = "Account budgets."
}

variable "anomaly_thresholds" {
  type = object({
    absolute_usd = number
    percentage   = number
  })
  description = "Cost anomaly notification thresholds."
}

variable "required_tags" {
  type        = list(string)
  description = "Mandatory cost allocation tag set."
  default     = ["plane", "region", "releaseId", "deployable"]
}

variable "tags" {
  type        = map(string)
  description = "Tags applied to everything in the backbone."
  default     = {}
}
