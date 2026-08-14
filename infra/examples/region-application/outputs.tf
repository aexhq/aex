output "service_arns" {
  value = {
    session-api = module.session_api.service_arn
    brain-mux   = module.brain_mux.service_arn
    tool-mux    = module.tool_mux.service_arn
  }
  description = "Exact regional session service to ECS service ARN."
}

output "function_alias_arns" {
  value       = { for k, m in module.function : k => m.alias_arn }
  description = "Exact focused regional worker to immutable Lambda alias ARN."
}

output "private_service_hostnames" {
  value       = module.service_discovery.hostnames
  description = "Private DNS names for Brain Mux and Tool Mux."
}

output "public_dns_name" {
  value       = module.public_lb.dns_name
  description = "DNS name of the public session load balancer."
}

output "role_arns" {
  value       = { for k, m in module.role : k => m.role_arn }
  description = "Exact regional deployable to execution role ARN."
}
