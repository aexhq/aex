output "role_arn" {
  value       = aws_iam_role.this.arn
  description = "ARN of the role a workflow job assumes."
}

output "role_name" {
  value       = var.role_name
  description = "Physical role name."
}

output "allowed_subjects" {
  value       = local.allowed_subjects
  description = "Exact OIDC subjects permitted to assume the role."
}
