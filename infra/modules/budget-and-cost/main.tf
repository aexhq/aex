resource "aws_budgets_budget" "this" {
  for_each = { for b in var.budgets : b.name => b }

  name         = each.value.name
  budget_type  = "COST"
  limit_amount = tostring(each.value.limit_amount)
  limit_unit   = each.value.limit_unit
  time_unit    = each.value.time_unit

  dynamic "cost_filter" {
    for_each = each.value.cost_filter_tags

    content {
      name   = "TagKeyValue"
      values = [for v in cost_filter.value : "aws:${cost_filter.key}$${v}"]
    }
  }

  notification {
    comparison_operator        = "GREATER_THAN"
    threshold                  = each.value.threshold_percent
    threshold_type             = "PERCENTAGE"
    notification_type          = "ACTUAL"
    subscriber_sns_topic_arns  = [var.sns_topic_arn]
    subscriber_email_addresses = []
  }

  notification {
    comparison_operator        = "GREATER_THAN"
    threshold                  = each.value.threshold_percent
    threshold_type             = "PERCENTAGE"
    notification_type          = "FORECASTED"
    subscriber_sns_topic_arns  = [var.sns_topic_arn]
    subscriber_email_addresses = []
  }
}

resource "aws_ce_anomaly_monitor" "this" {
  name              = "aex-service-anomalies"
  monitor_type      = "DIMENSIONAL"
  monitor_dimension = "SERVICE"
  tags              = var.tags
}

resource "aws_ce_anomaly_subscription" "this" {
  name      = "aex-anomaly-notifications"
  frequency = "IMMEDIATE"

  monitor_arn_list = [aws_ce_anomaly_monitor.this.arn]

  subscriber {
    type    = "SNS"
    address = var.sns_topic_arn
  }

  threshold_expression {
    or {
      dimension {
        key           = "ANOMALY_TOTAL_IMPACT_ABSOLUTE"
        match_options = ["GREATER_THAN_OR_EQUAL"]
        values        = [tostring(var.anomaly_thresholds.absolute_usd)]
      }
    }

    or {
      dimension {
        key           = "ANOMALY_TOTAL_IMPACT_PERCENTAGE"
        match_options = ["GREATER_THAN_OR_EQUAL"]
        values        = [tostring(var.anomaly_thresholds.percentage)]
      }
    }
  }

  tags = var.tags
}
