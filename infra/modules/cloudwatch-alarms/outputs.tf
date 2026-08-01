output "alarm_arns" {
  value       = { for k, a in aws_cloudwatch_metric_alarm.this : k => a.arn }
  description = "Alarm name to alarm ARN."
}

output "alarm_categories" {
  value       = { for a in var.alarm_specs : a.name => a.category }
  description = "Alarm name to category, for the routing rules that consume these alarms."
}
