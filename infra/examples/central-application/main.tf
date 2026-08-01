provider "aws" {
  region = var.region
}

locals {
  # The schedule targets are resolved from module outputs, never written down as
  # names. A schedule can therefore only ever point at an alias or a revisioned
  # task definition that this root actually created.
  schedule_targets = {
    finance_api  = module.finance_api.alias_arn
    schema_admin = module.schema_admin.task_definition_arn
  }
}

module "role" {
  source   = "../../modules/iam-deployable-role"
  for_each = var.deployable_grants

  deployable                  = each.key
  plane                       = var.plane
  region                      = var.region
  assume_principal            = each.value.assume_principal
  action_grants               = each.value.action_grants
  wildcard_resource_allowlist = each.value.wildcard_resource_allowlist
  tags                        = var.tags
}

module "database" {
  source = "../../modules/aurora-serverless-v2"

  cluster_identifier     = var.database.cluster_identifier
  database_name          = var.database.database_name
  master_username        = var.database.master_username
  engine_version         = var.database.engine_version
  min_acu                = var.database.min_acu
  max_acu                = var.database.max_acu
  backup_retention_days  = var.database.backup_retention_days
  admin_secret_arn       = var.database.admin_secret_arn
  region                 = var.region
  kms_key_arn            = var.kms_key_arn
  subnet_ids             = var.subnet_ids
  vpc_security_group_ids = var.security_group_ids
  tags                   = var.tags
}

module "settlement_queue" {
  source = "../../modules/sqs-queue"

  name                      = var.settlement_queue.name
  visibility_timeout        = var.settlement_queue.visibility_timeout
  max_receive_count         = var.settlement_queue.max_receive_count
  message_retention_seconds = var.settlement_queue.message_retention_seconds
  kms_key_arn               = var.kms_key_arn

  dlq = {
    enabled                   = true
    message_retention_seconds = var.settlement_queue.dlq_retention_seconds
  }

  tags = var.tags
}

module "finance_api" {
  source = "../../modules/lambda-function"

  function_name           = var.finance_api.function_name
  artifact_bucket         = var.artifact_bucket
  artifact_key            = var.finance_api.artifact_key
  artifact_object_version = var.finance_api.artifact_object_version
  artifact_sha256         = var.finance_api.artifact_sha256
  memory_mb               = var.finance_api.memory_mb
  timeout_s               = var.finance_api.timeout_s
  log_retention_days      = var.finance_api.log_retention_days
  env                     = var.finance_api.env
  role_arn                = module.role["finance-api"].role_arn
  tags                    = var.tags
}

module "settlement_consumer" {
  source = "../../modules/lambda-event-source"

  function_alias_arn = module.finance_api.alias_arn
  source_arn         = module.settlement_queue.arn
  batch_size         = var.settlement_queue.batch_size
  tags               = var.tags
}

module "schema_admin" {
  source = "../../modules/ecs-task-oneshot"

  family             = var.schema_admin.family
  image              = var.schema_admin.image
  cpu                = var.schema_admin.cpu
  memory             = var.schema_admin.memory
  role_arn           = module.role["central-schema-admin"].role_arn
  execution_role_arn = var.schema_admin.execution_role_arn
  subnets            = var.subnet_ids
  log_group_name     = var.schema_admin.log_group_name
  region             = var.region
  secret_env         = var.schema_admin.secret_env
  tags               = var.tags
}

module "schedules" {
  source = "../../modules/eventbridge-scheduler"

  group_name = var.schedule_group_name

  schedules = [
    for s in var.schedules : {
      name                    = s.name
      description             = s.description
      expression              = s.expression
      flexible_window_minutes = s.flexible_window_minutes
      target_arn              = local.schedule_targets[s.target_kind]
      role_arn                = s.role_arn
      input                   = s.input
    }
  ]
}
