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

module "function" {
  source   = "../../modules/lambda-function"
  for_each = var.lambda_units

  function_name           = each.value.function_name
  artifact_bucket         = var.artifact_bucket
  artifact_key            = each.value.artifact_key
  artifact_object_version = each.value.artifact_object_version
  artifact_sha256         = each.value.artifact_sha256
  memory_mb               = each.value.memory_mb
  timeout_s               = each.value.timeout_s
  reserved_concurrency    = each.value.reserved_concurrency
  log_retention_days      = each.value.log_retention_days
  log_kms_key_arn         = var.kms_key_arn
  env                     = each.value.env
  role_arn                = module.role[each.key].role_arn
  tags                    = var.tags
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
  name   = var.cluster_name
  tags   = var.tags
}

module "service_discovery" {
  source = "../../modules/service-discovery-private"

  namespace = var.service_discovery_namespace
  vpc_id    = var.vpc_id
  services  = ["brain-mux", "tool-mux"]
  tags      = var.tags
}

module "service_log" {
  source   = "../../modules/log-group"
  for_each = var.services

  name           = each.value.log_group_name
  retention_days = each.value.log_retention_days
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

module "session_target" {
  source = "../../modules/alb-service-target"

  name                 = "${var.alb.name}-session"
  listener_arn         = module.public_lb.listener_arn
  vpc_id               = var.vpc_id
  target_port          = var.services["session-api"].container_port
  rules                = var.session_rules
  deregistration_delay = module.public_lb.deregistration_delay
  tags                 = var.tags
}

module "session_api" {
  source = "../../modules/ecs-service"

  name                              = "session-api"
  task_definition_family            = "aex-${var.plane}-${var.region}-session-api"
  cluster_arn                       = module.cluster.arn
  cluster_name                      = var.cluster_name
  image                             = var.services["session-api"].image
  cpu                               = var.services["session-api"].cpu
  memory                            = var.services["session-api"].memory
  desired_count                     = var.services["session-api"].desired_count
  stop_timeout                      = var.services["session-api"].stop_timeout
  container_port                    = var.services["session-api"].container_port
  deregistration_delay              = module.session_target.deregistration_delay
  target_group_arn                  = module.session_target.target_group_arn
  health_check_grace_period_seconds = var.services["session-api"].health_check_grace_period_seconds
  autoscaling_bounds                = var.services["session-api"].autoscaling_bounds
  autoscaling_metrics               = var.services["session-api"].autoscaling_metrics
  env = merge(var.services["session-api"].env, {
    AEX_SESSION_API_BRAIN_URL = "http://${module.service_discovery.hostnames["brain-mux"]}:${var.services["brain-mux"].container_port}"
  })
  task_role_arn                        = module.role["session-api"].role_arn
  execution_role_arn                   = var.services["session-api"].execution_role_arn
  subnets                              = var.private_subnet_ids
  log_group_name                       = module.service_log["session-api"].name
  region                               = var.region
  vpc_id                               = var.vpc_id
  load_balancer_security_group_ids     = [module.public_lb.security_group_id]
  interface_endpoint_security_group_id = var.interface_endpoint_security_group_id
  gateway_endpoint_prefix_list_ids     = var.gateway_endpoint_prefix_list_ids
  tags                                 = var.tags
}

module "brain_mux" {
  source = "../../modules/ecs-service"

  name                   = "brain-mux"
  task_definition_family = "aex-${var.plane}-${var.region}-brain-mux"
  cluster_arn            = module.cluster.arn
  cluster_name           = var.cluster_name
  image                  = var.services["brain-mux"].image
  cpu                    = var.services["brain-mux"].cpu
  memory                 = var.services["brain-mux"].memory
  desired_count          = var.services["brain-mux"].desired_count
  stop_timeout           = var.services["brain-mux"].stop_timeout
  container_port         = var.services["brain-mux"].container_port
  autoscaling_bounds     = var.services["brain-mux"].autoscaling_bounds
  autoscaling_metrics    = var.services["brain-mux"].autoscaling_metrics
  env = merge(var.services["brain-mux"].env, {
    AEX_TOOL_MUX_URL = "http://${module.service_discovery.hostnames["tool-mux"]}:${var.services["tool-mux"].container_port}"
  })
  task_role_arn                        = module.role["brain-mux"].role_arn
  execution_role_arn                   = var.services["brain-mux"].execution_role_arn
  subnets                              = var.private_subnet_ids
  log_group_name                       = module.service_log["brain-mux"].name
  region                               = var.region
  vpc_id                               = var.vpc_id
  client_security_group_ids            = [module.session_api.security_group_id]
  interface_endpoint_security_group_id = var.interface_endpoint_security_group_id
  gateway_endpoint_prefix_list_ids     = var.gateway_endpoint_prefix_list_ids
  service_discovery_arn                = module.service_discovery.service_arns["brain-mux"]
  public_https_egress                  = true
  tags                                 = var.tags
}

module "tool_mux" {
  source = "../../modules/ecs-service"

  name                                 = "tool-mux"
  task_definition_family               = "aex-${var.plane}-${var.region}-tool-mux"
  cluster_arn                          = module.cluster.arn
  cluster_name                         = var.cluster_name
  image                                = var.services["tool-mux"].image
  cpu                                  = var.services["tool-mux"].cpu
  memory                               = var.services["tool-mux"].memory
  desired_count                        = var.services["tool-mux"].desired_count
  stop_timeout                         = var.services["tool-mux"].stop_timeout
  container_port                       = var.services["tool-mux"].container_port
  autoscaling_bounds                   = var.services["tool-mux"].autoscaling_bounds
  autoscaling_metrics                  = var.services["tool-mux"].autoscaling_metrics
  env                                  = var.services["tool-mux"].env
  task_role_arn                        = module.role["tool-mux"].role_arn
  execution_role_arn                   = var.services["tool-mux"].execution_role_arn
  subnets                              = var.private_subnet_ids
  log_group_name                       = module.service_log["tool-mux"].name
  region                               = var.region
  vpc_id                               = var.vpc_id
  client_security_group_ids            = [module.brain_mux.security_group_id]
  interface_endpoint_security_group_id = var.interface_endpoint_security_group_id
  gateway_endpoint_prefix_list_ids     = var.gateway_endpoint_prefix_list_ids
  service_discovery_arn                = module.service_discovery.service_arns["tool-mux"]
  public_https_egress                  = false
  tags                                 = var.tags
}

locals {
  regional_compute_units = setunion(toset(keys(var.services)), toset(keys(var.lambda_units)))
}
