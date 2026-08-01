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
  bucket_name_suffix = var.content_bucket_suffix
  kms_key_arn        = module.authority_key[var.content_authority].key_arn
  lifecycle_role_arn = var.content_lifecycle_role_arn
  tags               = var.tags
}
