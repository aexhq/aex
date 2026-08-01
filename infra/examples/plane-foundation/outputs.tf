output "artifact_key_arn" {
  value       = module.artifact_key.key_arn
  description = "Plane-wide key that encrypts published artifacts and registries."
}

output "repository_urls" {
  value       = { for k, m in module.repository : k => m.repository_url }
  description = "Repository name to URL. Images are consumed by digest only."
}

output "deploy_role_arn" {
  value       = module.deploy_role.role_arn
  description = "Role the deployment lane assumes. It carries no publish action."
}

output "alarm_arns" {
  value       = module.alarms.alarm_arns
  description = "Plane-wide alarm name to ARN."
}
