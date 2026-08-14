provider "aws" {
  region = var.region
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
  boundary_policy_arn         = var.permissions_boundary_policy_arn
  tags                        = var.tags
}

module "database" {
  source = "../../modules/aurora-serverless-v2"

  cluster_identifier        = var.database.cluster_identifier
  database_name             = var.database.database_name
  master_username           = var.database.master_username
  engine_version            = var.database.engine_version
  min_acu                   = var.database.min_acu
  max_acu                   = var.database.max_acu
  backup_retention_days     = var.database.backup_retention_days
  final_snapshot_identifier = var.database.final_snapshot_identifier
  region                    = var.region
  kms_key_arn               = var.kms_key_arn
  subnet_ids                = var.private_subnet_ids
  vpc_security_group_ids    = var.security_group_ids
  tags                      = var.tags
}

module "function" {
  source   = "../../modules/lambda-function"
  for_each = var.lambda_units

  function_name               = each.value.function_name
  artifact_bucket             = var.artifact_bucket
  artifact_key                = each.value.artifact_key
  artifact_object_version     = each.value.artifact_object_version
  artifact_sha256             = each.value.artifact_sha256
  runtime                     = each.value.runtime
  handler                     = each.value.handler
  memory_mb                   = each.value.memory_mb
  timeout_s                   = each.value.timeout_s
  reserved_concurrency        = each.value.reserved_concurrency
  log_retention_days          = each.value.log_retention_days
  log_kms_key_arn             = var.kms_key_arn
  env                         = each.value.env
  role_arn                    = module.role[each.key].role_arn
  public_function_url_enabled = each.value.public_function_url_enabled
  tags                        = var.tags
}

module "event_source" {
  source   = "../../modules/lambda-event-source"
  for_each = var.event_sources

  function_alias_arn  = module.function[each.value.unit].alias_arn
  source_arn          = each.value.source_arn
  batch_size          = each.value.batch_size
  max_batching_window = each.value.max_batching_window
  starting_position   = each.value.starting_position
  tags                = var.tags
}

module "cluster" {
  source = "../../modules/ecs-cluster"

  name = var.cluster_name
  tags = var.tags
}

module "control_log_group" {
  source = "../../modules/log-group"

  name           = var.control_api.log_group_name
  retention_days = var.control_api.log_retention_days
  kms_key_arn    = var.kms_key_arn
  tags           = var.tags
}

module "public_lb" {
  source = "../../modules/alb-public"

  name               = var.alb.name
  vpc_id             = var.vpc_id
  subnet_ids         = var.public_subnet_ids
  certificate_arn    = var.alb.certificate_arn
  access_logs_bucket = var.alb.access_logs_bucket
  tags               = var.tags
}

module "control_target" {
  source = "../../modules/alb-service-target"

  name                 = "${var.alb.name}-control"
  listener_arn         = module.public_lb.listener_arn
  vpc_id               = var.vpc_id
  target_port          = var.control_api.container_port
  rules                = var.control_api.rules
  deregistration_delay = module.public_lb.deregistration_delay
  tags                 = var.tags
}

module "control_service" {
  source = "../../modules/ecs-service"

  name                                 = var.control_api.name
  task_definition_family               = "aex-${var.plane}-${var.region}-${var.control_api.name}"
  cluster_arn                          = module.cluster.arn
  cluster_name                         = var.cluster_name
  image                                = var.control_api.image
  cpu                                  = var.control_api.cpu
  memory                               = var.control_api.memory
  desired_count                        = var.control_api.desired_count
  stop_timeout                         = var.control_api.stop_timeout
  container_port                       = var.control_api.container_port
  deregistration_delay                 = module.control_target.deregistration_delay
  target_group_arn                     = module.control_target.target_group_arn
  health_check_grace_period_seconds    = var.control_api.health_check_grace_period_seconds
  autoscaling_bounds                   = var.control_api.autoscaling_bounds
  autoscaling_metrics                  = var.control_api.autoscaling_metrics
  env                                  = var.control_api.env
  task_role_arn                        = module.role["control-api"].role_arn
  execution_role_arn                   = var.control_api.execution_role_arn
  subnets                              = var.private_subnet_ids
  log_group_name                       = module.control_log_group.name
  region                               = var.region
  vpc_id                               = var.vpc_id
  load_balancer_security_group_ids     = [module.public_lb.security_group_id]
  interface_endpoint_security_group_id = var.interface_endpoint_security_group_id
  gateway_endpoint_prefix_list_ids     = var.gateway_endpoint_prefix_list_ids
  tags                                 = var.tags
}

locals {
  central_compute_units = setunion(toset(keys(var.lambda_units)), toset(["control-api"]))
}
