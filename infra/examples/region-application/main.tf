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

module "session_api" {
  source = "../../modules/lambda-function"

  function_name           = var.session_api.function_name
  artifact_bucket         = var.artifact_bucket
  artifact_key            = var.session_api.artifact_key
  artifact_object_version = var.session_api.artifact_object_version
  artifact_sha256         = var.session_api.artifact_sha256
  memory_mb               = var.session_api.memory_mb
  timeout_s               = var.session_api.timeout_s
  log_retention_days      = var.session_api.log_retention_days
  env                     = var.session_api.env
  role_arn                = module.role["regional-session-api"].role_arn
  tags                    = var.tags
}

module "public_lb" {
  source = "../../modules/alb-public"

  name               = var.alb.name
  vpc_id             = var.vpc_id
  subnet_ids         = var.public_subnet_ids
  security_group_ids = var.alb_security_group_ids
  certificate_arn    = var.alb.certificate_arn
  target_port        = var.stream_service.container_port
  access_logs_bucket = var.alb.access_logs_bucket
  tags               = var.tags
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
  stop_timeout   = var.stream_service.stop_timeout
  container_port = var.stream_service.container_port

  deregistration_delay = module.public_lb.deregistration_delay
  target_group_arn     = module.public_lb.target_group_arn

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
