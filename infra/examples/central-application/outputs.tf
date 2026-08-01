output "finance_api_alias_arn" {
  value       = module.finance_api.alias_arn
  description = "Alias every caller and event source targets."
}

output "settlement_queue_arn" {
  value       = module.settlement_queue.arn
  description = "Settlement work queue."
}

output "settlement_dlq_arn" {
  value       = module.settlement_queue.dlq_arn
  description = "Settlement dead-letter queue."
}

output "database_cluster_arn" {
  value       = module.database.cluster_arn
  description = "Data API `resourceArn` for the finance cluster."
}

output "schema_admin_task_definition_arn" {
  value       = module.schema_admin.task_definition_arn
  description = "Revisioned task definition the migration runner is invoked with."
}

output "role_arns" {
  value       = { for k, m in module.role : k => m.role_arn }
  description = "Deployable to execution role ARN."
}

output "schedule_arns" {
  value       = module.schedules.schedule_arns
  description = "Schedule name to schedule ARN."
}
