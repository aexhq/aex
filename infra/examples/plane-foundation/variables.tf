variable "plane" {
  type        = string
  description = "Deployment plane this foundation belongs to."
}

variable "region" {
  type        = string
  description = "Region the registries and alarms live in."
}

variable "artifact_key" {
  type = object({
    alias                     = string
    description               = string
    encryption_context_equals = map(string)
    policy_statements = list(object({
      sid                          = string
      effect                       = string
      principal_type               = string
      principals                   = list(string)
      actions                      = list(string)
      resources                    = list(string)
      data_plane                   = bool
      encryption_context_workspace = optional(string)
      conditions = optional(list(object({
        test     = string
        variable = string
        values   = list(string)
      })), [])
    }))
  })
  description = "The plane-wide key that encrypts published artifacts, and its complete policy. Every principal is named by the caller."
}

variable "repositories" {
  type = map(object({
    untagged_expire_days = number
  }))
  description = "Container repositories to create, keyed by repository name under the `aex/` namespace."
}

variable "github_role_name" {
  type        = string
  description = "Exact plane-scoped name of the GitHub deploy role."
}

variable "github_repository" {
  type        = string
  description = "The one repository whose workflows may assume the deploy role."
}

variable "github_oidc_provider_arn" {
  type        = string
  description = "ARN of the account's GitHub OIDC provider."
}

variable "github_allowed_refs" {
  type        = list(string)
  description = "Exact refs permitted to assume the deploy role."
}

variable "github_allowed_workflows" {
  type        = list(string)
  description = "Exact workflow paths permitted to assume the deploy role."
}

variable "permission_profiles" {
  type        = map(list(string))
  description = "Actions granted by each of the four permission profiles."
}

variable "profile_resources" {
  type        = map(list(string))
  description = "Resources each profile's actions apply to."
}

variable "ops_topic_arn" {
  type        = string
  description = "Ops topic created by the account backbone."
}

variable "alarm_specs" {
  type = list(object({
    name                = string
    description         = string
    owner               = string
    urgency             = string
    runbook_url         = string
    action_class        = string
    category            = string
    namespace           = string
    metric_name         = string
    statistic           = string
    period              = number
    evaluation_periods  = number
    datapoints_to_alarm = optional(number)
    threshold           = number
    comparison_operator = string
    treat_missing_data  = string
    dimensions          = optional(map(string), {})
  }))
  description = "Plane-wide alarms."
}

variable "tags" {
  type        = map(string)
  description = "Tags applied to everything in the plane foundation."
  default     = {}
}
