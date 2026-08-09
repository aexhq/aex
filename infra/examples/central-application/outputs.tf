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

output "central_api_service_arn" {
  value       = module.central_api_service.service_arn
  description = "The merged central HTTP service."
}

output "public_dns_name" {
  value       = module.public_lb.dns_name
  description = "The load balancer's own name. It answers permanently: an ALB has no `disable_execute_api_endpoint` analogue, and `alb-service-target` emits `path_pattern` conditions only, so there is no host-header rule that could close it. The web ACL below is associated with the load balancer rather than the listener partly for that reason."
}

output "device_flow_rate_limit_arn" {
  value       = module.device_flow_rate_limit.web_acl_arn
  description = "The web ACL replacing the API Gateway throttle on the two unauthenticated routes."
}

output "device_flow_rate_limit_metric" {
  value       = module.device_flow_rate_limit.rule_metric_name
  description = "The dimension `BlockedRequests` is published under, so an alarm names the string the ACL does rather than re-deriving it."
}

output "schedule_arns" {
  value       = module.schedules.schedule_arns
  description = "Schedule name to schedule ARN."
}
