output "session_stream_service_arn" {
  value       = module.session_stream_service.service_arn
  description = "The merged regional request-path service, serving both the finite session API and the NDJSON stream behind the public load balancer."
}

output "operation_queue_arn" {
  value       = module.operation_queue.arn
  description = "Session operation queue."
}

output "pipe_arn" {
  value       = module.journal_hint_pipe.pipe_arn
  description = "Pipe carrying journal mutations to the operation queue as hints."
}

output "public_dns_name" {
  value       = module.public_lb.dns_name
  description = "DNS name of the public load balancer."
}

output "role_arns" {
  value       = { for k, m in module.role : k => m.role_arn }
  description = "Deployable to execution role ARN."
}
