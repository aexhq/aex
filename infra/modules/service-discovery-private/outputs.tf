output "namespace_id" {
  value       = aws_service_discovery_private_dns_namespace.this.id
  description = "The Cloud Map namespace id."
}

output "namespace_name" {
  value       = aws_service_discovery_private_dns_namespace.this.name
  description = "The DNS suffix every registered service is reached under."
}

output "service_arns" {
  value       = { for name, service in aws_service_discovery_service.this : name => service.arn }
  description = "Cloud Map service ARN per registered name, for `ecs-service`'s `service_discovery_arn`."
}

output "hostnames" {
  value       = { for name, service in aws_service_discovery_service.this : name => "${service.name}.${aws_service_discovery_private_dns_namespace.this.name}" }
  description = "The hostname per registered name, for the caller's configuration. This is the only address these services have."
}
