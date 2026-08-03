provider "aws" {
  region = var.region
}

module "artifact_key" {
  source = "../../modules/kms-key"

  alias                     = var.artifact_key.alias
  description               = var.artifact_key.description
  encryption_context_equals = var.artifact_key.encryption_context_equals
  policy_statements         = var.artifact_key.policy_statements
  tags                      = var.tags
}

module "repository" {
  source   = "../../modules/ecr-repository"
  for_each = var.repositories

  name        = each.key
  kms_key_arn = module.artifact_key.key_arn

  lifecycle_by_reference = {
    untagged_expire_days = each.value.untagged_expire_days
  }

  tags = var.tags
}

module "deploy_role" {
  source = "../../modules/github-oidc-role"

  role_name           = var.github_role_name
  repository          = var.github_repository
  oidc_provider_arn   = var.github_oidc_provider_arn
  allowed_refs        = var.github_allowed_refs
  allowed_workflows   = var.github_allowed_workflows
  permissions_profile = "deploy"
  permission_profiles = var.permission_profiles
  profile_resources   = var.profile_resources
  tags                = var.tags
}

module "alarms" {
  source = "../../modules/cloudwatch-alarms"

  alarm_specs   = var.alarm_specs
  sns_topic_arn = var.ops_topic_arn
  tags          = var.tags
}
