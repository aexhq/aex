output "function_arn" {
  value       = aws_lambda_function.this.arn
  description = "ARN of the unqualified function. Callers should use `alias_arn`."
}

output "version" {
  value       = aws_lambda_function.this.version
  description = "Published version minted by this deployment."
}

output "alias_arn" {
  value       = aws_lambda_alias.this.arn
  description = "ARN of the alias every event source and caller targets."
}

output "log_group_name" {
  value       = aws_cloudwatch_log_group.this.name
  description = "Name of the function's log group."
}
