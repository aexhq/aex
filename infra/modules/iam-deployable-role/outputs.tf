output "role_arn" {
  value       = aws_iam_role.this.arn
  description = "ARN of the deployable role."
}

output "role_name" {
  value       = local.role_name
  description = "Physical name of the deployable role."
}
