output "cluster_arn" {
  value       = aws_rds_cluster.this.arn
  description = "ARN of the cluster. Data API callers pass this as `resourceArn`."
}

output "endpoint" {
  value       = aws_rds_cluster.this.endpoint
  description = "Writer endpoint."
}

output "reader_endpoint" {
  value       = aws_rds_cluster.this.reader_endpoint
  description = "Reader endpoint. With no reader instance at launch it resolves to the writer."
}

output "admin_secret_arn" {
  value       = var.admin_secret_arn
  description = "Secret ARN Data API callers pass as `secretArn`."
}

output "managed_master_user_secret_arn" {
  value       = try(aws_rds_cluster.this.master_user_secret[0].secret_arn, null)
  description = "ARN of the secret RDS mints and rotates for the master user."
}
