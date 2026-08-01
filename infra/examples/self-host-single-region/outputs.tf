output "vpc_id" {
  value       = module.network.vpc_id
  description = "Id of the VPC."
}

output "table_names" {
  value       = module.tables.table_names
  description = "Logical table name to physical table name."
}

output "content_bucket" {
  value       = module.content.bucket
  description = "Content bucket."
}

output "artifact_bucket" {
  value       = module.artifacts.bucket
  description = "Bucket the published artifacts are copied into."
}

output "ops_topic_arn" {
  value       = module.ops_topic.topic_arn
  description = "Topic operational notifications go to."
}

output "session_api_alias_arn" {
  value       = module.session_api.alias_arn
  description = "Alias callers target for the session API."
}
