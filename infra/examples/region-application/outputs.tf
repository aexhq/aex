output "session_api_alias_arn" {
  value       = module.session_api.alias_arn
  description = "Alias every caller targets for the regional session API."
}

output "operation_queue_arn" {
  value       = module.operation_queue.arn
  description = "Session operation queue."
}

output "pipe_arn" {
  value       = module.journal_hint_pipe.pipe_arn
  description = "Pipe carrying journal mutations to the operation queue as hints."
}

output "stream_service_arn" {
  value       = module.stream_service.service_arn
  description = "The regional stream service."
}

output "public_dns_name" {
  value       = module.public_lb.dns_name
  description = "DNS name of the public load balancer."
}

output "role_arns" {
  value       = { for k, m in module.role : k => m.role_arn }
  description = "Deployable to execution role ARN."
}
