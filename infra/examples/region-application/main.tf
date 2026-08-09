provider "aws" {
  region = var.region
}

module "cluster" {
  source = "../../modules/ecs-cluster"

  name = var.cluster_name
  tags = var.tags
}

module "stream_log_group" {
  source = "../../modules/log-group"

  name           = var.stream_service.log_group_name
  retention_days = var.stream_service.log_retention_days
  kms_key_arn    = var.kms_key_arn
  tags           = var.tags
}

module "session_log_group" {
  source = "../../modules/log-group"

  name           = var.session_api.log_group_name
  retention_days = var.session_api.log_retention_days
  kms_key_arn    = var.kms_key_arn
  tags           = var.tags
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

module "operation_queue" {
  source = "../../modules/sqs-queue"

  name                      = var.operation_queue.name
  visibility_timeout        = var.operation_queue.visibility_timeout
  max_receive_count         = var.operation_queue.max_receive_count
  message_retention_seconds = var.operation_queue.message_retention_seconds
  kms_key_arn               = var.kms_key_arn

  dlq = {
    enabled                   = true
    message_retention_seconds = var.operation_queue.dlq_retention_seconds
  }

  tags = var.tags
}

module "journal_hint_pipe" {
  source = "../../modules/dynamodb-stream-pipe"

  name               = var.stream_pipe.name
  source_stream_arn  = var.session_journal_stream_arn
  target_queue_arn   = module.operation_queue.arn
  target_kms_key_arn = var.kms_key_arn
  filter_pattern     = var.stream_pipe.filter_pattern
  input_template     = var.stream_pipe.input_template
  batch_size         = var.stream_pipe.batch_size
  tags               = var.tags
}

module "public_lb" {
  source = "../../modules/alb-public"

  name               = var.alb.name
  subnet_ids         = var.public_subnet_ids
  security_group_ids = var.alb_security_group_ids
  certificate_arn    = var.alb.certificate_arn
  access_logs_bucket = var.alb.access_logs_bucket
  tags               = var.tags
}

# Two services, two target groups, and as many rules each as their pattern sets
# need. The lower number is evaluated first, so a narrower rule must hold a
# lower priority than any rule that would also match it. `regional-stream` is
# given the 10s and `regional-session-api` the 20s, which leaves each service
# room to add rules without reaching into the other's band.
module "stream_target" {
  source = "../../modules/alb-service-target"

  name                 = "${var.alb.name}-stream"
  listener_arn         = module.public_lb.listener_arn
  vpc_id               = var.vpc_id
  target_port          = var.stream_service.container_port
  rules                = var.stream_service.rules
  deregistration_delay = module.public_lb.deregistration_delay
  tags                 = var.tags
}

module "session_target" {
  source = "../../modules/alb-service-target"

  name                 = "${var.alb.name}-session"
  listener_arn         = module.public_lb.listener_arn
  vpc_id               = var.vpc_id
  target_port          = var.session_api.container_port
  rules                = var.session_api.rules
  deregistration_delay = module.public_lb.deregistration_delay
  tags                 = var.tags
}

module "stream_service" {
  source = "../../modules/ecs-service"

  name                   = var.stream_service.name
  task_definition_family = "aex-${var.plane}-${var.region}-${var.stream_service.name}"
  cluster_arn            = module.cluster.arn
  cluster_name           = var.cluster_name

  image          = var.stream_service.image
  cpu            = var.stream_service.cpu
  memory         = var.stream_service.memory
  desired_count  = var.stream_service.desired_count
  stop_timeout   = var.stream_service.stop_timeout
  container_port = var.stream_service.container_port

  # Both come from the target group in front of this service, so the load
  # balancer and the task cannot disagree about the drain window.
  deregistration_delay = module.stream_target.deregistration_delay
  target_group_arn     = module.stream_target.target_group_arn

  health_check_grace_period_seconds = var.stream_service.health_check_grace_period_seconds

  autoscaling_bounds  = var.stream_service.autoscaling_bounds
  autoscaling_metrics = var.stream_service.autoscaling_metrics

  env                = var.stream_service.env
  task_role_arn      = module.role["regional-stream"].role_arn
  execution_role_arn = var.stream_service.execution_role_arn
  subnets            = var.private_subnet_ids
  security_group_ids = var.service_security_group_ids
  log_group_name     = module.stream_log_group.name
  region             = var.region

  tags = var.tags
}

module "session_service" {
  source = "../../modules/ecs-service"

  name                   = var.session_api.name
  task_definition_family = "aex-${var.plane}-${var.region}-${var.session_api.name}"
  cluster_arn            = module.cluster.arn
  cluster_name           = var.cluster_name

  image          = var.session_api.image
  cpu            = var.session_api.cpu
  memory         = var.session_api.memory
  desired_count  = var.session_api.desired_count
  stop_timeout   = var.session_api.stop_timeout
  container_port = var.session_api.container_port

  deregistration_delay = module.session_target.deregistration_delay
  target_group_arn     = module.session_target.target_group_arn

  health_check_grace_period_seconds = var.session_api.health_check_grace_period_seconds

  autoscaling_bounds  = var.session_api.autoscaling_bounds
  autoscaling_metrics = var.session_api.autoscaling_metrics

  env                = var.session_api.env
  task_role_arn      = module.role["regional-session-api"].role_arn
  execution_role_arn = var.session_api.execution_role_arn
  subnets            = var.private_subnet_ids
  security_group_ids = var.service_security_group_ids
  log_group_name     = module.session_log_group.name
  region             = var.region

  tags = var.tags
}
