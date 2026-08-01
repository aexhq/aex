variable "required_tags" {
  type        = list(string)
  description = "The mandatory tag set every budget filters on. Without all four, a cost line cannot be attributed to a plane, a region, a release or a deployable, and a cost anomaly becomes an unanswerable question."

  validation {
    condition     = length(setsubtract(["plane", "region", "releaseId", "deployable"], var.required_tags)) == 0
    error_message = "The mandatory tag set must include `plane`, `region`, `releaseId` and `deployable`."
  }

  validation {
    condition     = alltrue([for t in var.required_tags : can(regex("^[A-Za-z][A-Za-z0-9_-]*$", t))])
    error_message = "Every tag key must be a valid cost allocation tag key."
  }
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
  description = "The budgets to create. Each filters on the mandatory tags so the alert names something actionable."

  validation {
    condition     = length(var.budgets) > 0
    error_message = "At least one budget is required."
  }

  validation {
    condition     = length(distinct([for b in var.budgets : b.name])) == length(var.budgets)
    error_message = "Budget names must be unique."
  }

  validation {
    condition     = alltrue([for b in var.budgets : b.limit_amount > 0])
    error_message = "Every budget limit must be positive."
  }

  validation {
    condition     = alltrue([for b in var.budgets : contains(["USD"], b.limit_unit)])
    error_message = "Budgets are denominated in USD."
  }

  validation {
    condition     = alltrue([for b in var.budgets : contains(["DAILY", "MONTHLY", "QUARTERLY", "ANNUALLY"], b.time_unit)])
    error_message = "The time unit must be a Budgets time unit."
  }

  validation {
    condition     = alltrue([for b in var.budgets : b.threshold_percent > 0 && b.threshold_percent <= 200])
    error_message = "The notification threshold must be between 0 and 200 percent."
  }

  validation {
    condition = alltrue([
      for b in var.budgets : length(setsubtract(var.required_tags, keys(b.cost_filter_tags))) == 0
    ])
    error_message = "Every budget must filter on the full mandatory tag set."
  }
}

variable "anomaly_thresholds" {
  type = object({
    absolute_usd = number
    percentage   = number
  })
  description = "When a detected anomaly is worth telling someone about."

  validation {
    condition     = var.anomaly_thresholds.absolute_usd > 0
    error_message = "The absolute anomaly threshold must be positive."
  }

  validation {
    condition     = var.anomaly_thresholds.percentage > 0 && var.anomaly_thresholds.percentage <= 1000
    error_message = "The percentage anomaly threshold must be between 0 and 1000."
  }
}

variable "sns_topic_arn" {
  type        = string
  description = "Topic budget and anomaly notifications publish to."

  validation {
    condition     = can(regex("^arn:aws[a-z-]*:sns:[a-z0-9-]+:[0-9A-Za-z-]{1,64}:[A-Za-z0-9_-]+$", var.sns_topic_arn))
    error_message = "The notification target must be an SNS topic ARN."
  }
}

variable "tags" {
  type        = map(string)
  default     = {}
  description = "Tags applied to the anomaly monitor and subscription."
}
