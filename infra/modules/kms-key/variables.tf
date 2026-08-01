variable "alias" {
  type        = string
  description = "KMS alias for the key, including the `alias/` prefix. The alias is the stable public handle; the key id is never a binding value."

  validation {
    condition     = can(regex("^alias/aex-[a-z0-9][a-z0-9-]*$", var.alias))
    error_message = "The alias must look like `alias/aex-<name>` with a lowercase, hyphen-separated name."
  }
}

variable "description" {
  type        = string
  description = "Human-readable purpose of the key, recorded on the key itself."

  validation {
    condition     = length(var.description) >= 8
    error_message = "The description must be at least 8 characters so an operator can tell keys apart."
  }
}

variable "rotation" {
  type        = bool
  default     = true
  description = "Whether automatic key-material rotation is enabled. Rotation is mandatory; the variable exists so the policy is asserted rather than assumed."

  validation {
    condition     = var.rotation
    error_message = "Automatic key rotation must be enabled; `rotation = false` is not an allowed configuration."
  }
}

variable "rotation_period_days" {
  type        = number
  default     = 365
  description = "Automatic rotation period in days."

  validation {
    condition     = var.rotation_period_days >= 90 && var.rotation_period_days <= 2560
    error_message = "The rotation period must be between 90 and 2560 days."
  }
}

variable "deletion_window_days" {
  type        = number
  default     = 30
  description = "Pending-deletion window in days. The floor is 30 so an accidental destroy is always recoverable for a full month."

  validation {
    condition     = var.deletion_window_days >= 30
    error_message = "The deletion window must be at least 30 days."
  }
}

variable "encryption_context_equals" {
  type        = map(string)
  default     = {}
  description = "Additional encryption-context key/value pairs every data-plane grant must match exactly. `aex:workspace` is supplied per statement and must not appear here."

  validation {
    condition     = alltrue([for k in keys(var.encryption_context_equals) : can(regex("^aex:[a-z][a-z0-9-]*$", k))])
    error_message = "Every encryption-context key must look like `aex:<name>`."
  }

  validation {
    condition     = !contains(keys(var.encryption_context_equals), "aex:workspace")
    error_message = "`aex:workspace` is a per-statement value and must be supplied through `policy_statements[*].encryption_context_workspace`."
  }
}

variable "policy_statements" {
  type = list(object({
    sid                          = string
    effect                       = string
    principal_type               = string
    principals                   = list(string)
    actions                      = list(string)
    resources                    = list(string)
    data_plane                   = bool
    encryption_context_workspace = optional(string)
  }))
  description = "The complete key policy. The module invents no statement of its own: the root supplies every grant, including the account administration grant. `data_plane = true` marks a grant used by product code on customer data."

  validation {
    condition     = length(var.policy_statements) > 0
    error_message = "A KMS key with an empty policy is unusable; supply at least the administration grant."
  }

  validation {
    condition     = alltrue([for s in var.policy_statements : contains(["Allow", "Deny"], s.effect)])
    error_message = "Every statement effect must be `Allow` or `Deny`."
  }

  validation {
    condition     = alltrue([for s in var.policy_statements : contains(["AWS", "Service", "Federated"], s.principal_type)])
    error_message = "Every statement principal type must be `AWS`, `Service` or `Federated`."
  }

  validation {
    condition     = alltrue([for s in var.policy_statements : length(s.principals) > 0 && !contains(s.principals, "*")])
    error_message = "A wildcard principal is not allowed in a key policy; name every principal explicitly."
  }

  validation {
    condition     = alltrue([for s in var.policy_statements : !contains(s.actions, "*")])
    error_message = "`Action: \"*\"` is not allowed in a key policy."
  }

  validation {
    condition = alltrue([
      for s in var.policy_statements : !(contains(s.actions, "kms:*") && contains(s.resources, "*"))
    ])
    error_message = "`kms:*` on `Resource: \"*\"` is not allowed; enumerate the actions or scope the resource."
  }

  validation {
    condition = alltrue([
      for s in var.policy_statements :
      !s.data_plane || try(length(s.encryption_context_workspace), 0) > 0
    ])
    error_message = "Every data-plane grant must carry a `kms:EncryptionContext:aex:workspace` condition; set `encryption_context_workspace`."
  }
}

variable "tags" {
  type        = map(string)
  default     = {}
  description = "Tags applied to the key."
}
