output "control_api_service_arn" {
  value       = module.control_service.service_arn
  description = "The session-MVP control API service."
}

output "function_alias_arns" {
  value       = { for k, m in module.function : k => m.alias_arn }
  description = "Exact focused central worker to immutable Lambda alias ARN."
}

output "stripe_webhook_url" {
  value       = module.function["stripe-webhook-edge"].public_function_url
  description = "Alias-qualified HTTPS ingress for verified Stripe webhooks."
}

output "database_cluster_arn" {
  value       = module.database.cluster_arn
  description = "Data API resource ARN for the central control and billing database."
}

output "role_arns" {
  value       = { for k, m in module.role : k => m.role_arn }
  description = "Exact central deployable to execution role ARN."
}

output "public_dns_name" {
  value       = module.public_lb.dns_name
  description = "DNS name of the public control load balancer."
}
