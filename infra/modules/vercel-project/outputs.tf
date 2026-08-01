output "project_id" {
  value       = vercel_project.this.id
  description = "Id of the Vercel project. The environment binding records it per plane."
}

output "project_name" {
  value       = local.full_project_name
  description = "Physical project name, including the plane suffix."
}

output "domain" {
  value       = vercel_project_domain.this.domain
  description = "Domain attached to the project."
}
