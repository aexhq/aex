output "task_definition_arn" {
  value       = aws_ecs_task_definition.this.arn
  description = "ARN of the task definition, including its revision. Callers pass the revisioned ARN to `RunTask`."
}

output "network_configuration" {
  value = {
    subnets          = var.subnets
    assign_public_ip = var.assign_public_ip
  }
  description = "Network configuration a caller passes to `RunTask`. The public address is always off."
}
