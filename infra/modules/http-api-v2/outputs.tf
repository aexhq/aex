output "api_id" {
  value       = aws_apigatewayv2_api.this.id
  description = "HTTP API id."
}

output "execution_arn" {
  value       = aws_apigatewayv2_api.this.execution_arn
  description = "Execution ARN used by exact Lambda invoke permissions."
}

output "url" {
  value       = "https://${var.domain_name}"
  description = "Canonical public URL. The raw execute-api endpoint is disabled."
}

output "route_keys" {
  value       = { for operation, route in var.routes : operation => route.route_key }
  description = "Exact operation-to-route map materialized by the module."
}

output "integration_routes" {
  value = {
    for owner in keys(var.integration_alias_arns) :
    owner => sort([for operation, route in var.routes : operation if route.integration == owner])
  }
  description = "Operation ids grouped by integration owner."
}

output "domain_handoff" {
  value = {
    host           = var.domain_name
    target         = aws_apigatewayv2_domain_name.this.domain_name_configuration[0].target_domain_name
    target_zone_id = aws_apigatewayv2_domain_name.this.domain_name_configuration[0].hosted_zone_id
  }
  description = "External-DNS handoff for the Regional custom domain. The module creates no DNS record."
}

output "access_log_group_name" {
  value       = aws_cloudwatch_log_group.access.name
  description = "CloudWatch access-log group."
}
