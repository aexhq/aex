variable "role_name" {
  type        = string
  description = "Exact plane-scoped IAM role name. The caller owns physical naming so two planes in one account cannot collide."

  validation {
    condition     = can(regex("^aex-[a-z0-9][a-z0-9-]{2,59}$", var.role_name))
    error_message = "The role name must be an exact 6-63 character lowercase `aex-...` name."
  }

  validation {
    condition     = !strcontains(var.role_name, "*")
    error_message = "The role name must be exact; wildcards are not physical identities."
  }
}

variable "repository" {
  type        = string
  description = "The one repository permitted to assume the role, as `owner/name`."

  validation {
    condition     = can(regex("^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$", var.repository))
    error_message = "The repository must be `owner/name`."
  }

  validation {
    condition     = !strcontains(var.repository, "*")
    error_message = "The repository must be exact; a wildcard would let any repository in the owner assume the role."
  }
}

variable "oidc_provider_arn" {
  type        = string
  description = "ARN of the account's GitHub OIDC provider. It is an account-level resource, created once by the account backbone."

  validation {
    condition     = can(regex("^arn:aws[a-z-]*:iam::[0-9A-Za-z-]{1,64}:oidc-provider/token\\.actions\\.githubusercontent\\.com$", var.oidc_provider_arn))
    error_message = "The provider must be the GitHub Actions OIDC provider ARN."
  }
}

variable "allowed_refs" {
  type        = list(string)
  description = "Full git refs permitted to assume the role, such as `refs/heads/main`. Exact refs only."

  validation {
    condition     = length(var.allowed_refs) > 0
    error_message = "At least one ref must be permitted."
  }

  validation {
    condition     = alltrue([for r in var.allowed_refs : startswith(r, "refs/")])
    error_message = "Every ref must be a full git ref such as `refs/heads/main`."
  }

  validation {
    condition     = alltrue([for r in var.allowed_refs : !strcontains(r, "*")])
    error_message = "A ref pattern is not allowed; name every ref exactly."
  }
}

variable "allowed_environments" {
  type        = list(string)
  default     = []
  description = "Exact protected GitHub Environment names. When non-empty, only environment subjects may assume the role; the plain ref subject is deliberately excluded."

  validation {
    condition     = alltrue([for environment in var.allowed_environments : can(regex("^aex-(dev|prd)$", environment))])
    error_message = "Every protected environment must be exactly `aex-dev` or `aex-prd`."
  }

  validation {
    condition     = alltrue([for environment in var.allowed_environments : !strcontains(environment, "*")])
    error_message = "An environment pattern is not allowed; name every protected environment exactly."
  }
}

variable "allowed_workflows" {
  type        = list(string)
  description = "Workflow file paths permitted to assume the role, such as `.github/workflows/main.yml`. Exact paths only."

  validation {
    condition     = length(var.allowed_workflows) > 0
    error_message = "At least one workflow path must be permitted."
  }

  validation {
    condition     = alltrue([for w in var.allowed_workflows : can(regex("^\\.github/workflows/[A-Za-z0-9._-]+\\.ya?ml$", w))])
    error_message = "Every workflow must be an exact path under `.github/workflows/`."
  }
}

variable "permissions_profile" {
  type        = string
  description = "Which profile this role carries. One role carries one profile; a role that could both publish and deploy would collapse the separation the profiles exist to create."

  validation {
    condition     = contains(["publish", "plan", "deploy", "readonly"], var.permissions_profile)
    error_message = "The profile must be `publish`, `plan`, `deploy` or `readonly`."
  }
}

variable "permission_profiles" {
  type        = map(list(string))
  description = "Actions granted by each profile. Supplied by the root so the profile contents are auditable in one place rather than buried in a module."

  validation {
    condition     = length(setsubtract(["publish", "plan", "deploy", "readonly"], keys(var.permission_profiles))) == 0
    error_message = "All four profiles must be defined: `publish`, `plan`, `deploy` and `readonly`."
  }

  validation {
    condition = length(setintersection(
      toset(lookup(var.permission_profiles, "publish", [])),
      toset(lookup(var.permission_profiles, "deploy", []))
    )) == 0
    error_message = "The `publish` and `deploy` profiles must be disjoint; an action in both would let one credential mint bytes and roll them out."
  }

  validation {
    condition = alltrue(flatten([
      for p, actions in var.permission_profiles : [for a in actions : !contains(["*"], a) && !endswith(a, ":*")]
    ]))
    error_message = "No profile may grant a wildcard action."
  }
}

variable "profile_resources" {
  type        = map(list(string))
  description = "Resources each profile's actions apply to."

  validation {
    condition     = length(setsubtract(["publish", "plan", "deploy", "readonly"], keys(var.profile_resources))) == 0
    error_message = "All four profiles must declare their resources."
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
