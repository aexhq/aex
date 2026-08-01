output "bucket" {
  value       = local.bucket_name
  description = "Physical name of the content bucket."
}

output "bucket_arn" {
  value       = local.bucket_arn
  description = "ARN of the content bucket."
}
