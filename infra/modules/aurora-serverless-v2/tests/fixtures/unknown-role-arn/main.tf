resource "terraform_data" "role" {
  input = "arn:aws:iam::000000000000:role/aex-dev-aurora-control-wake"
}

output "role_arn" {
  value       = terraform_data.role.output
  description = "A valid role ARN whose value remains unknown during a plan-only test run."
}
