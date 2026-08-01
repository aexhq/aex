output "service_arn" {
  value       = aws_ecs_service.this.id
  description = "ARN of the service."
}

output "task_definition_arn" {
  value       = aws_ecs_task_definition.this.arn
  description = "Revisioned ARN of the task definition the service runs."
}

output "deregistration_delay" {
  value       = var.deregistration_delay
  description = "Drain window the caller must configure on the target group in front of this service."
}
