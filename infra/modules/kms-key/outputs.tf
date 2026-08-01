output "key_arn" {
  value       = aws_kms_key.this.arn
  description = "ARN of the customer-managed key."
}

output "alias_arn" {
  value       = aws_kms_alias.this.arn
  description = "ARN of the alias that names the key."
}
