variable "deployable" {
  type        = string
  description = "The deployable this role belongs to, exactly as it is named in `release/units.toml`. One role, one deployable, no sharing."

  validation {
    condition     = can(regex("^[a-z][a-z0-9-]{2,63}$", var.deployable))
    error_message = "The deployable id must be lowercase, hyphen-separated and 3-64 characters."
  }
}

variable "plane" {
  type        = string
  description = "Deployment plane. Every scopable statement carries this value as a condition."

  validation {
    condition     = contains(["dev", "prd"], var.plane)
    error_message = "The plane must be `dev` or `prd`."
  }
}

variable "region" {
  type        = string
  description = "AWS region. Every scopable statement carries this value as a condition."

  validation {
    condition     = can(regex("^[a-z]{2}-[a-z]+-[0-9]$", var.region))
    error_message = "The region must be an AWS region code such as `eu-west-1`."
  }
}

variable "assume_principal" {
  type = object({
    type        = string
    identifiers = list(string)
  })
  description = "Who may assume the role. `type` is `Service`, `AWS` or `Federated`."

  validation {
    condition     = contains(["Service", "AWS", "Federated"], var.assume_principal.type)
    error_message = "The assume principal type must be `Service`, `AWS` or `Federated`."
  }

  validation {
    condition     = length(var.assume_principal.identifiers) > 0 && !contains(var.assume_principal.identifiers, "*")
    error_message = "The trust policy must name at least one principal and may not use a wildcard."
  }
}

variable "action_grants" {
  type = list(object({
    sid              = string
    actions          = list(string)
    resources        = list(string)
    scopable         = bool
    condition_key    = optional(string)
    condition_values = optional(list(string))
  }))
  description = "The permission set, normally taken from the regional bundle's generated IAM action lists. `scopable = true` means the resource type supports a plane/region condition, and the module then requires one."

  validation {
    condition     = length(var.action_grants) > 0
    error_message = "A deployable role with no grants is a defect, not a hardening measure; give it the grants it needs."
  }

  validation {
    condition     = alltrue([for g in var.action_grants : !contains(g.actions, "*")])
    error_message = "`Action: \"*\"` is not allowed in a deployable role."
  }

  validation {
    condition = alltrue([
      for g in var.action_grants : alltrue([
        for a in g.actions : can(regex("^[a-z0-9-]+:[A-Za-z0-9*]+$", a))
      ])
    ])
    error_message = "Every action must be `service:Action`; a bare service wildcard such as `s3:*` is not an action."
  }

  validation {
    condition = alltrue([
      for g in var.action_grants : alltrue([
        for a in g.actions : !endswith(a, ":*")
      ])
    ])
    error_message = "A whole-service wildcard such as `dynamodb:*` is not allowed; enumerate the actions."
  }

  validation {
    condition     = alltrue([for g in var.action_grants : length(g.resources) > 0])
    error_message = "Every grant must name at least one resource."
  }

  validation {
    condition = alltrue([
      for g in var.action_grants :
      !contains(g.resources, "*") || alltrue([for a in g.actions : contains(var.wildcard_resource_allowlist, a)])
    ])
    error_message = "`Resource: \"*\"` is only permitted for actions in `wildcard_resource_allowlist`."
  }

  validation {
    condition = alltrue([
      for g in var.action_grants :
      !g.scopable || (try(length(g.condition_key), 0) > 0 && try(length(g.condition_values), 0) > 0)
    ])
    error_message = "Every grant whose resource type supports scoping must carry a plane or region condition."
  }
}

variable "wildcard_resource_allowlist" {
  type        = list(string)
  description = "The explicit, exhaustive list of actions permitted on `Resource: \"*\"`. These are the actions AWS defines with no resource, such as `kms:GenerateRandom`."

  validation {
    condition     = !contains(var.wildcard_resource_allowlist, "*")
    error_message = "The allowlist must enumerate actions; it may not itself be a wildcard."
  }

  validation {
    condition     = length(var.wildcard_resource_allowlist) <= 12
    error_message = "At most twelve actions may be allowlisted for `Resource: \"*\"`."
  }
}

variable "boundary_policy_arn" {
  type        = string
  default     = null
  description = "Optional permissions boundary attached to the role."

  validation {
    condition     = var.boundary_policy_arn == null || can(regex("^arn:aws[a-z-]*:iam::[0-9A-Za-z-]{1,64}:policy/[A-Za-z0-9+=,.@_/-]+$", coalesce(var.boundary_policy_arn, "none")))
    error_message = "When set, the permissions boundary must be an IAM policy ARN."
  }
}

variable "max_session_duration" {
  type        = number
  default     = 3600
  description = "Maximum session duration in seconds."

  validation {
    condition     = var.max_session_duration >= 900 && var.max_session_duration <= 43200
    error_message = "The maximum session duration must be between 900 and 43200 seconds."
  }
}

variable "tags" {
  type        = map(string)
  default     = {}
  description = "Tags applied to the role."
}
