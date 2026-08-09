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

output "session_service_arn" {
  value       = module.session_service.service_arn
  description = "The session API service. This root stands up no public edge, so a self-hoster puts their own ingress in front of it."
}

output "cluster_arn" {
  value       = module.cluster.arn
  description = "ECS cluster the session service runs in."
}
