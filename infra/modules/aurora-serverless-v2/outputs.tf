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
  description = "Reader endpoint. With reader_count zero it resolves to the writer."
}

output "managed_master_user_secret_arn" {
  value       = one(aws_rds_cluster.this.master_user_secret).secret_arn
  description = "ARN of the secret RDS mints and rotates for the master user. Data API callers pass this exact ARN as `secretArn`."
}
