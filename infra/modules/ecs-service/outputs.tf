output "service_arn" {
  value       = one(concat(aws_ecs_service.autoscaled[*].id, aws_ecs_service.static[*].id))
  description = "ARN of the service, whichever variant this configuration materializes."
}

output "task_definition_arn" {
  value       = aws_ecs_task_definition.this.arn
  description = "Revisioned ARN of the task definition the service runs."
}

output "security_group_id" {
  value       = aws_security_group.task.id
  description = "The tasks' own security group. It admits the load balancer in front of this service on the container port and nothing else, and its egress is TLS to the private AWS endpoints only."
}

output "deregistration_delay" {
  value       = var.deregistration_delay
  description = "Drain window the caller must configure on the target group in front of this service."
}
