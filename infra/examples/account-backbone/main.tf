provider "aws" {
  region = var.region
}

module "tfstate" {
  source = "../../modules/tfstate-backend"

  plane              = var.plane
  bucket_name_suffix = var.state_bucket_suffix
  kms_alias          = var.state_kms_alias
  tags               = var.tags
}

module "artifacts" {
  source = "../../modules/artifact-bucket"

  plane              = var.plane
  bucket_name_suffix = var.artifact_bucket_suffix
  retention_days     = var.artifact_retention_days
  tags               = var.tags
}

module "ops_topic" {
  source = "../../modules/sns-ops-topic"

  name               = var.ops_topic_name
  region             = var.region
  account_id         = var.account_id
  publish_principals = var.ops_publish_principals
  subscriptions      = var.ops_subscriptions
  tags               = var.tags
}

module "publish_role" {
  source = "../../modules/github-oidc-role"

  repository          = var.github_repository
  oidc_provider_arn   = var.github_oidc_provider_arn
  allowed_refs        = var.github_allowed_refs
  allowed_workflows   = var.github_allowed_workflows
  permissions_profile = "publish"
  permission_profiles = var.permission_profiles
  profile_resources   = var.profile_resources
  tags                = var.tags
}

module "cost" {
  source = "../../modules/budget-and-cost"

  required_tags      = var.required_tags
  budgets            = var.budgets
  anomaly_thresholds = var.anomaly_thresholds
  sns_topic_arn      = module.ops_topic.topic_arn
  tags               = var.tags
}
