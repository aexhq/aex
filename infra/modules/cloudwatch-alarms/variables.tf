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
  description = "The alarms to create. Every alarm names an owner and a runbook, and declares whether it reports a correctness failure or a loss of telemetry, because those two demand opposite responses."

  validation {
    condition     = length(var.alarm_specs) > 0
    error_message = "At least one alarm is required."
  }

  validation {
    condition     = length(distinct([for a in var.alarm_specs : a.name])) == length(var.alarm_specs)
    error_message = "Alarm names must be unique."
  }

  validation {
    condition = alltrue([
      for a in var.alarm_specs : contains([
        "contracts", "central-identity", "central-finance", "regional-domains",
        "regional-stores", "regional-services", "brain-core", "providers",
        "tools-mcp", "hands", "observations-usage", "clients",
        "infrastructure", "delivery", "test-architecture",
      ], a.owner)
    ])
    error_message = "Every alarm owner must be one of the declared streams."
  }

  validation {
    condition     = alltrue([for a in var.alarm_specs : can(regex("^https://", a.runbook_url))])
    error_message = "Every alarm must carry an https runbook URL."
  }

  validation {
    condition     = alltrue([for a in var.alarm_specs : contains(["page", "ticket", "record"], a.urgency)])
    error_message = "Urgency must be `page`, `ticket` or `record`."
  }

  validation {
    condition     = alltrue([for a in var.alarm_specs : contains(["none", "notify", "auto_mitigate"], a.action_class)])
    error_message = "The automatic-action class must be `none`, `notify` or `auto_mitigate`."
  }

  validation {
    condition = alltrue([
      for a in var.alarm_specs :
      contains(["correctness", "telemetry_loss", "saturation", "cost"], a.category)
    ])
    error_message = "The category must be `correctness`, `telemetry_loss`, `saturation` or `cost`."
  }

  validation {
    condition = alltrue([
      for a in var.alarm_specs :
      contains(["SampleCount", "Average", "Sum", "Minimum", "Maximum"], a.statistic)
    ])
    error_message = "The statistic must be a CloudWatch statistic."
  }

  validation {
    condition = alltrue([
      for a in var.alarm_specs : contains([
        "GreaterThanOrEqualToThreshold", "GreaterThanThreshold",
        "LessThanThreshold", "LessThanOrEqualToThreshold",
      ], a.comparison_operator)
    ])
    error_message = "The comparison operator must be a CloudWatch comparison operator."
  }

  validation {
    condition = alltrue([
      for a in var.alarm_specs : contains(["missing", "ignore", "breaching", "notBreaching"], a.treat_missing_data)
    ])
    error_message = "The missing-data treatment must be a CloudWatch value."
  }

  validation {
    condition = alltrue([
      for a in var.alarm_specs : a.category != "telemetry_loss" || a.treat_missing_data == "breaching"
    ])
    error_message = "A telemetry-loss alarm must treat missing data as breaching; silence is exactly the condition it exists to catch."
  }

  validation {
    condition     = alltrue([for a in var.alarm_specs : a.period >= 10 && a.evaluation_periods >= 1])
    error_message = "The period must be at least 10 seconds and there must be at least one evaluation period."
  }
}

variable "sns_topic_arn" {
  type        = string
  description = "Topic every alarm action publishes to."

  validation {
    condition     = can(regex("^arn:aws[a-z-]*:sns:[a-z0-9-]+:[0-9A-Za-z-]{1,64}:[A-Za-z0-9_-]+$", var.sns_topic_arn))
    error_message = "The alarm action must be an SNS topic ARN."
  }
}

variable "tags" {
  type        = map(string)
  default     = {}
  description = "Tags applied to every alarm, merged with the per-alarm classification tags."
}
