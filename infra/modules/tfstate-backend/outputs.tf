output "bucket" {
  value       = local.bucket_name
  description = "Physical name of the state bucket. Roots name it in an `s3` backend block with `use_lockfile = true`."
}

output "kms_key_arn" {
  value       = aws_kms_key.state.arn
  description = "ARN of the key that encrypts state."
}

output "kms_alias_arn" {
  value       = aws_kms_alias.state.arn
  description = "ARN of the state key alias."
}
