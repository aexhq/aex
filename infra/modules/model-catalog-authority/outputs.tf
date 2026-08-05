output "kms_key_arn" {
  description = "Exact asymmetric KMS key ARN for the protected publisher."
  value       = aws_kms_key.publisher.arn
}

output "kms_key_alias" {
  description = "Human-discoverable alias; workflows must use kms_key_arn instead."
  value       = aws_kms_alias.publisher.name
}

output "publisher_role_arn" {
  description = "Exact GitHub OIDC role ARN for the protected publisher."
  value       = aws_iam_role.publisher.arn
}

output "logical_key_id" {
  description = "Stable public keyId emitted in trust roots and signature records."
  value       = var.logical_key_id
}

output "oidc_subject" {
  description = "Exact protected-environment subject admitted by the role."
  value       = "repo:${var.repository}:environment:${var.environment}"
}
