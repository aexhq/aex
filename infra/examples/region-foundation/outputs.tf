output "vpc_id" {
  value       = module.network.vpc_id
  description = "Id of the regional VPC."
}

output "private_subnet_ids" {
  value       = module.network.subnet_ids.private
  description = "Private subnets every workload runs in."
}

output "authority_key_arns" {
  value       = { for k, m in module.authority_key : k => m.key_arn }
  description = "Authority to customer-managed key ARN."
}

output "table_names" {
  value       = module.tables.table_names
  description = "Logical table name to physical table name."
}

output "stream_arns" {
  value       = module.tables.stream_arns
  description = "Logical table name to stream ARN."
}

output "content_bucket" {
  value       = module.content.bucket
  description = "Regional content bucket."
}
