output "state_bucket" {
  value       = module.tfstate.bucket
  description = "Bucket every other root names in its `s3` backend block."
}

output "state_kms_key_arn" {
  value       = module.tfstate.kms_key_arn
  description = "Key that encrypts Terraform state."
}

output "artifact_bucket" {
  value       = module.artifacts.bucket
  description = "Bucket the publish lane writes Lambda ZIPs and module bundles to."
}

output "ops_topic_arn" {
  value       = module.ops_topic.topic_arn
  description = "Topic alarms and budgets publish to."
}

output "publish_role_arn" {
  value       = module.publish_role.role_arn
  description = "Role the publication lane assumes. It carries no deploy action."
}

output "budget_arns" {
  value       = module.cost.budget_arns
  description = "Budget name to budget ARN."
}
