output "budget_arns" {
  value       = { for k, b in aws_budgets_budget.this : k => b.arn }
  description = "Budget name to budget ARN."
}

output "anomaly_monitor_arn" {
  value       = aws_ce_anomaly_monitor.this.arn
  description = "ARN of the cost anomaly monitor."
}

output "required_tags" {
  value       = var.required_tags
  description = "The mandatory tag set every deployable must carry for its cost to be attributable."
}
