variable "team_id" {
  type        = string
  description = "Vercel team the project belongs to. Supplied by the environment binding; the module never discovers it."

  validation {
    condition     = length(var.team_id) > 0
    error_message = "A team id is required."
  }
}

variable "project_name" {
  type        = string
  description = "Base project name. The plane suffix is appended by the module, so `dev` and `prd` are always two separate Vercel projects."

  validation {
    condition     = can(regex("^[a-z][a-z0-9-]{1,80}$", var.project_name))
    error_message = "The project name must be lowercase and hyphen-separated."
  }

  validation {
    condition     = !endswith(var.project_name, "-dev") && !endswith(var.project_name, "-prd")
    error_message = "Do not put the plane in the base name; the module appends it, and a doubled suffix would let two planes collide on one project."
  }
}

variable "plane" {
  type        = string
  description = "Deployment plane. It becomes the project name suffix."

  validation {
    condition     = contains(["dev", "prd"], var.plane)
    error_message = "The plane must be `dev` or `prd`."
  }
}

variable "domain" {
  type        = string
  description = "Domain attached to the project."

  validation {
    condition     = can(regex("^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)+$", var.domain))
    error_message = "The domain must be a hostname."
  }
}

variable "auto_assign_domains" {
  type        = bool
  default     = false
  description = "Whether Vercel automatically points production domains at the newest deployment. It does not: promotion is a release decision recorded in a manifest, not a side effect of a push."

  validation {
    condition     = var.auto_assign_domains == false
    error_message = "Production domain auto-assignment must stay off; promotion is an explicit release step."
  }
}

variable "build_command" {
  type        = string
  default     = null
  description = "Build command. It stays unset: the artifact is a prebuilt Build Output API tree whose digest is already in the manifest, and a rebuild here would produce bytes nothing has attested."

  validation {
    condition     = var.build_command == null
    error_message = "No build command may be configured; the deployment is prebuilt and a rebuild would mint unattested bytes."
  }
}

variable "framework" {
  type        = string
  default     = null
  description = "Framework preset. It stays null for the same reason as `build_command`: a preset implies a build."

  validation {
    condition     = var.framework == null
    error_message = "No framework preset may be configured; a preset implies a build step."
  }
}

variable "env_vars" {
  type = list(object({
    key       = string
    target    = list(string)
    reference = string
    comment   = optional(string, "")
  }))
  default     = []
  description = "Runtime environment variables. Every value is a reference handle - a secret ARN or a parameter path - never a credential."

  validation {
    condition     = alltrue([for e in var.env_vars : can(regex("^(AEX|NEXT_PUBLIC)_[A-Z0-9_]+$", e.key))])
    error_message = "Every environment key must match `^(AEX|NEXT_PUBLIC)_[A-Z0-9_]+$`."
  }

  validation {
    condition = alltrue(flatten([
      for e in var.env_vars : [for t in e.target : contains(["production", "preview", "development"], t)]
    ]))
    error_message = "Every target must be `production`, `preview` or `development`."
  }

  validation {
    condition     = alltrue([for e in var.env_vars : length(e.target) > 0])
    error_message = "Every environment variable must name at least one target."
  }

  validation {
    condition = alltrue([
      for e in var.env_vars : !can(regex("^(AKIA|ASIA|sk-|sk_live|pk_live|ghp_|github_pat_|-----BEGIN)", e.reference))
    ])
    error_message = "A value looks like a credential. Environment values are reference handles resolved at runtime, never secrets."
  }

  validation {
    condition     = length(distinct([for e in var.env_vars : e.key])) == length(var.env_vars)
    error_message = "Environment keys must be unique."
  }
}

variable "serverless_function_region" {
  type        = string
  default     = "dub1"
  description = "Vercel region the functions run in."

  validation {
    condition     = can(regex("^[a-z]{3}[0-9]$", var.serverless_function_region))
    error_message = "The region must be a Vercel region code such as `dub1`."
  }
}
