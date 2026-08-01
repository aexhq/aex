provider "aws" {
  region = var.region
}

module "network" {
  source = "../../modules/vpc-regional"

  name               = var.vpc.name
  region             = var.region
  cidr               = var.vpc.cidr
  az_count           = var.vpc.az_count
  availability_zones = var.vpc.availability_zones
  endpoints          = var.vpc.endpoints
  tags               = var.tags
}

module "authority_key" {
  source   = "../../modules/kms-key"
  for_each = var.authority_keys

  alias                     = each.value.alias
  description               = each.value.description
  encryption_context_equals = each.value.encryption_context_equals
  policy_statements         = each.value.policy_statements
  tags                      = var.tags
}

module "tables" {
  source = "../../modules/regional-dynamodb-tables"

  plane       = var.plane
  region      = var.region
  name_prefix = var.name_prefix

  table_definitions           = var.table_definitions
  table_definitions_digest    = var.table_definitions_digest
  expected_definitions_digest = var.expected_definitions_digest
  keystore_physical_name      = var.keystore_physical_name

  kms_key_arn_by_authority = { for k, m in module.authority_key : k => m.key_arn }

  tags = var.tags
}

module "content" {
  source = "../../modules/content-bucket"

  plane              = var.plane
  region             = var.region
  bucket_name_suffix = var.bucket_suffix
  kms_key_arn        = module.authority_key[var.content_authority].key_arn
  lifecycle_role_arn = var.content_lifecycle_role_arn
  tags               = var.tags
}

module "artifacts" {
  source = "../../modules/artifact-bucket"

  plane              = var.plane
  bucket_name_suffix = var.bucket_suffix
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

module "session_api_role" {
  source = "../../modules/iam-deployable-role"

  deployable                  = "regional-session-api"
  plane                       = var.plane
  region                      = var.region
  assume_principal            = var.session_api_grants.assume_principal
  action_grants               = var.session_api_grants.action_grants
  wildcard_resource_allowlist = var.session_api_grants.wildcard_resource_allowlist
  tags                        = var.tags
}

module "session_api" {
  source = "../../modules/lambda-function"

  function_name           = var.session_api.function_name
  artifact_bucket         = module.artifacts.bucket
  artifact_key            = var.session_api.artifact_key
  artifact_object_version = var.session_api.artifact_object_version
  artifact_sha256         = var.session_api.artifact_sha256
  memory_mb               = var.session_api.memory_mb
  timeout_s               = var.session_api.timeout_s
  log_retention_days      = var.session_api.log_retention_days
  env                     = var.session_api.env
  role_arn                = module.session_api_role.role_arn
  tags                    = var.tags
}
