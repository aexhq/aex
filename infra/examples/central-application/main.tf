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
  subnet_ids                = var.subnet_ids
  vpc_security_group_ids    = var.security_group_ids
  tags                      = var.tags
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
  stop_timeout       = var.schema_admin.stop_timeout
  role_arn           = module.role["central-schema-admin"].role_arn
  execution_role_arn = var.schema_admin.execution_role_arn
  subnets            = var.subnet_ids
  log_group_name     = var.schema_admin.log_group_name
  region             = var.region
  secret_env         = var.schema_admin.secret_env
  tags               = var.tags
}

# --- the public edge ----------------------------------------------------------
#
# New to this root. The central plane served its whole public surface from three
# Lambdas behind an API Gateway, so it had no load balancer, no cluster and no
# public subnets. `central-api` merges those three into one long-lived task, and
# a task needs all three.
#
# The `alb-public` + `alb-service-target` + `ecs-service` triad is the same shape
# `region-application` already uses, module for module and argument for argument.
# What is different here is the fourth module: the regional plane never had a
# gateway throttle to lose.

module "cluster" {
  source = "../../modules/ecs-cluster"

  name = var.cluster_name
  tags = var.tags
}

module "central_api_log_group" {
  source = "../../modules/log-group"

  name           = var.central_api.log_group_name
  retention_days = var.central_api.log_retention_days
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

# One service, one target group. The gateway forwarded per operation, with a
# `aws_lambda_permission` narrowed to one method and one path each; a listener
# rule forwards a prefix and nothing below it is separately authorised. Route
# level authorisation is entirely the process's own `Action::central` and
# `requirement()` table now, which is where it always actually lived — the
# gateway's per-route permissions were a second, weaker copy of it.
module "central_api_target" {
  source = "../../modules/alb-service-target"

  name                 = "${var.alb.name}-edge"
  listener_arn         = module.public_lb.listener_arn
  vpc_id               = var.vpc_id
  target_port          = var.central_api.container_port
  rules                = var.central_api.rules
  deregistration_delay = module.public_lb.deregistration_delay
  tags                 = var.tags
}

module "central_api_service" {
  source = "../../modules/ecs-service"

  name                   = var.central_api.name
  task_definition_family = "aex-${var.plane}-${var.region}-${var.central_api.name}"
  cluster_arn            = module.cluster.arn
  cluster_name           = var.cluster_name

  image          = var.central_api.image
  cpu            = var.central_api.cpu
  memory         = var.central_api.memory
  desired_count  = var.central_api.desired_count
  stop_timeout   = var.central_api.stop_timeout
  container_port = var.central_api.container_port

  # Both come from the target group in front of this service, so the load
  # balancer and the task cannot disagree about the drain window.
  deregistration_delay = module.central_api_target.deregistration_delay
  target_group_arn     = module.central_api_target.target_group_arn

  health_check_grace_period_seconds = var.central_api.health_check_grace_period_seconds

  autoscaling_bounds  = var.central_api.autoscaling_bounds
  autoscaling_metrics = var.central_api.autoscaling_metrics

  env                = var.central_api.env
  task_role_arn      = module.role["central-api"].role_arn
  execution_role_arn = var.central_api.execution_role_arn
  subnets            = var.subnet_ids
  log_group_name     = module.central_api_log_group.name
  region             = var.region

  # One task group for one deployable. The load balancer's group is the only
  # source it admits, and the two endpoint handles are the whole of what it may
  # reach outbound.
  vpc_id                               = var.vpc_id
  load_balancer_security_group_ids     = [module.public_lb.security_group_id]
  interface_endpoint_security_group_id = var.interface_endpoint_security_group_id
  gateway_endpoint_prefix_list_ids     = var.gateway_endpoint_prefix_list_ids

  tags = var.tags
}

# The one thing this root gains that `region-application` has no equivalent of.
#
# `http-api-v2` carried `throttling_burst_limit = 100` and
# `throttling_rate_limit = 50`, which was the only request-rate control anywhere
# in the central plane. An ALB has none. This is not a like-for-like replacement
# — a rate-based rule counts per source IP over a fixed window rather than an
# aggregate rate — and it is pointed only at the two routes that carry no
# credential.
#
# It associates with the load balancer, not with the listener: AWS attaches a
# REGIONAL web ACL to the load balancer, so it also covers the ALB's own
# `*.elb.amazonaws.com` name, which stays resolvable because
# `alb-service-target` emits `path_pattern` conditions only and there is no
# `disable_execute_api_endpoint` analogue to close it.
module "device_flow_rate_limit" {
  source = "../../modules/waf-rate-limit"

  name         = var.device_flow_rate_limit.name
  resource_arn = module.public_lb.arn
  rate_limit   = var.device_flow_rate_limit.rate_limit
  paths        = var.device_flow_rate_limit.paths
  tags         = var.tags
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
