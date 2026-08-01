resource "aws_cloudwatch_metric_alarm" "this" {
  for_each = { for a in var.alarm_specs : a.name => a }

  alarm_name        = each.value.name
  alarm_description = "${each.value.description} Runbook: ${each.value.runbook_url}"

  namespace           = each.value.namespace
  metric_name         = each.value.metric_name
  statistic           = each.value.statistic
  period              = each.value.period
  evaluation_periods  = each.value.evaluation_periods
  datapoints_to_alarm = each.value.datapoints_to_alarm
  threshold           = each.value.threshold
  comparison_operator = each.value.comparison_operator
  treat_missing_data  = each.value.treat_missing_data
  dimensions          = each.value.dimensions

  alarm_actions             = [var.sns_topic_arn]
  ok_actions                = [var.sns_topic_arn]
  insufficient_data_actions = each.value.category == "telemetry_loss" ? [var.sns_topic_arn] : []

  # The category tag is what separates "the product computed the wrong answer"
  # from "we stopped hearing from the product". They demand opposite responses,
  # so they must never be routed by the same rule.
  tags = merge(var.tags, {
    "aex:alarm-owner"        = each.value.owner
    "aex:alarm-urgency"      = each.value.urgency
    "aex:alarm-category"     = each.value.category
    "aex:alarm-action-class" = each.value.action_class
    "aex:runbook"            = each.value.runbook_url
  })
}
