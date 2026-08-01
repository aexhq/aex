variable "group_name" {
  type        = string
  description = "Schedule group every schedule is created in."

  validation {
    condition     = can(regex("^aex-[a-z0-9-]{3,50}$", var.group_name))
    error_message = "The group name must start with `aex-` and be lowercase and hyphen-separated."
  }
}

variable "schedules" {
  type = list(object({
    name                     = string
    description              = string
    expression               = string
    expression_timezone      = optional(string, "UTC")
    flexible_window_minutes  = number
    target_arn               = string
    role_arn                 = string
    input                    = string
    dead_letter_arn          = optional(string)
    maximum_retry_attempts   = optional(number, 3)
    maximum_event_age_in_sec = optional(number, 3600)
  }))
  description = "The schedules to create. Each target is either a Lambda alias or a task definition pinned to a revision; a mutable name is rejected."

  validation {
    condition     = length(var.schedules) > 0
    error_message = "At least one schedule is required."
  }

  validation {
    condition     = length(distinct([for s in var.schedules : s.name])) == length(var.schedules)
    error_message = "Schedule names must be unique."
  }

  validation {
    condition     = alltrue([for s in var.schedules : can(regex("^[a-z][a-z0-9-]{2,63}$", s.name))])
    error_message = "Every schedule name must be lowercase and hyphen-separated."
  }

  validation {
    condition = alltrue([
      for s in var.schedules :
      can(regex("^(rate\\([0-9]+ (minute|minutes|hour|hours|day|days)\\)|cron\\(.+\\)|at\\(.+\\))$", s.expression))
    ])
    error_message = "Every expression must be a `rate(...)`, `cron(...)` or `at(...)` expression."
  }

  validation {
    condition = alltrue([
      for s in var.schedules :
      can(regex("^arn:aws[a-z-]*:lambda:[a-z0-9-]+:[0-9A-Za-z-]{1,64}:function:[A-Za-z0-9_-]+:[A-Za-z0-9_-]+$", s.target_arn))
      || can(regex("^arn:aws[a-z-]*:ecs:[a-z0-9-]+:[0-9A-Za-z-]{1,64}:task-definition/[A-Za-z0-9_-]+:[0-9]+$", s.target_arn))
    ])
    error_message = "Every target must be a qualified Lambda alias ARN or an ECS task definition ARN pinned to a revision; a mutable name is not a target."
  }

  validation {
    condition     = alltrue([for s in var.schedules : !endswith(s.target_arn, ":$LATEST")])
    error_message = "A schedule may not target `$LATEST`."
  }

  validation {
    condition     = alltrue([for s in var.schedules : s.flexible_window_minutes >= 0 && s.flexible_window_minutes <= 1440])
    error_message = "The flexible window must be between 0 and 1440 minutes."
  }

  validation {
    condition     = alltrue([for s in var.schedules : can(jsondecode(s.input))])
    error_message = "Every schedule input must be a JSON document."
  }

  validation {
    condition     = alltrue([for s in var.schedules : can(regex("^arn:aws[a-z-]*:iam::[0-9A-Za-z-]{1,64}:role/[A-Za-z0-9+=,.@_/-]+$", s.role_arn))])
    error_message = "Every schedule must name an IAM role ARN to invoke its target with."
  }
}

variable "kms_key_arn" {
  type        = string
  default     = null
  description = "Optional customer-managed key for schedule payload encryption."

  validation {
    condition     = var.kms_key_arn == null || can(regex("^arn:aws[a-z-]*:kms:", coalesce(var.kms_key_arn, "none")))
    error_message = "When set, the key must be a KMS key ARN."
  }
}
