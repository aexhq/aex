provider "aws" {
  region = var.region
}

module "cluster" {
  source = "../../modules/ecs-cluster"

  name = var.cluster_name
  tags = var.tags
}

# One log group. `regional-session-api` and `regional-stream` merged into
# `session-stream-api`, so the unary and NDJSON halves write to one stream and an
# operator correlating a request with the socket it opened no longer has to join
# two groups by timestamp.
module "session_stream_log_group" {
  source = "../../modules/log-group"

  name           = var.session_stream_api.log_group_name
  retention_days = var.session_stream_api.log_retention_days
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
  vpc_id             = var.vpc_id
  subnet_ids         = var.public_subnet_ids
  certificate_arn    = var.alb.certificate_arn
  access_logs_bucket = var.alb.access_logs_bucket
  tags               = var.tags
}

# One service, one target group, and as many rules as the merged pattern set
# needs. Every regional first path segment has exactly one serving artifact, so
# no two rules here can both match one request and evaluation order decides
# nothing; a priority is an address, not a tie-break. Were a pattern ever added
# that a second rule could also match, the lower number would win, so a narrower
# rule would have to hold a lower priority than the wider one.
#
# `/api/streams` and the unary prefixes used to be split across two target groups
# because they were two deployables. They are one now, so the split would only
# have cost a second group and a second idle task pair. The prefixes remain
# disjoint in the contract, which is what would let a future root put them back
# behind two target groups without touching a route.
module "session_stream_target" {
  source = "../../modules/alb-service-target"

  name                 = "${var.alb.name}-edge"
  listener_arn         = module.public_lb.listener_arn
  vpc_id               = var.vpc_id
  target_port          = var.session_stream_api.container_port
  rules                = var.session_stream_api.rules
  deregistration_delay = module.public_lb.deregistration_delay
  tags                 = var.tags
}

module "session_stream_service" {
  source = "../../modules/ecs-service"

  name                   = var.session_stream_api.name
  task_definition_family = "aex-${var.plane}-${var.region}-${var.session_stream_api.name}"
  cluster_arn            = module.cluster.arn
  cluster_name           = var.cluster_name

  image          = var.session_stream_api.image
  cpu            = var.session_stream_api.cpu
  memory         = var.session_stream_api.memory
  desired_count  = var.session_stream_api.desired_count
  stop_timeout   = var.session_stream_api.stop_timeout
  container_port = var.session_stream_api.container_port

  # Both come from the target group in front of this service, so the load
  # balancer and the task cannot disagree about the drain window.
  deregistration_delay = module.session_stream_target.deregistration_delay
  target_group_arn     = module.session_stream_target.target_group_arn

  health_check_grace_period_seconds = var.session_stream_api.health_check_grace_period_seconds

  autoscaling_bounds  = var.session_stream_api.autoscaling_bounds
  autoscaling_metrics = var.session_stream_api.autoscaling_metrics

  env                = var.session_stream_api.env
  task_role_arn      = module.role["session-stream-api"].role_arn
  execution_role_arn = var.session_stream_api.execution_role_arn
  subnets            = var.private_subnet_ids
  log_group_name     = module.session_stream_log_group.name
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

# --- the platform tool executor ----------------------------------------------
#
# The first service in this root with no public edge, and the thing that had to
# be built for it was an address rather than a load balancer.
#
# `ecs-service` already deploys a service with no target group -- the variable
# defaults to null and the `load_balancer` block is conditional -- so the gap the
# placement record recorded ("the module requires a target group") is not there
# any more, and an internal-ALB module was not needed. What was missing is that a
# Fargate task's address changes on every deployment, so `brain-mux` needs a name.
# Cloud Map publishes one; a load balancer would have solved a name-resolution
# problem with a listener, a target group, two more security-group edges and a
# monthly bill.
module "tool_executor_log_group" {
  source = "../../modules/log-group"

  name           = var.tool_executor.log_group_name
  retention_days = var.tool_executor.log_retention_days
  kms_key_arn    = var.kms_key_arn
  tags           = var.tags
}

module "internal_names" {
  source = "../../modules/service-discovery-private"

  namespace = var.internal_namespace
  vpc_id    = var.vpc_id
  services  = ["tool-executor"]
  tags      = var.tags
}

module "tool_executor_service" {
  source = "../../modules/ecs-service"

  name                   = var.tool_executor.name
  task_definition_family = "aex-${var.plane}-${var.region}-${var.tool_executor.name}"
  cluster_arn            = module.cluster.arn
  cluster_name           = var.cluster_name

  image          = var.tool_executor.image
  cpu            = var.tool_executor.cpu
  memory         = var.tool_executor.memory
  desired_count  = var.tool_executor.desired_count
  stop_timeout   = var.tool_executor.stop_timeout
  container_port = var.tool_executor.container_port

  # No target group, and therefore no grace period: ECS rejects one on a service
  # with no load balancer, and `ecs-service` refuses the combination rather than
  # passing it through to be rejected at apply.
  service_discovery_arn = module.internal_names.service_arns["tool-executor"]

  # The only source the tasks admit. Until `brain-mux` has Terraform of its own
  # this list is empty, and an empty list is a service that admits nothing --
  # which is the fail-closed direction and is stated here rather than worked
  # around, because the alternative is a private service reachable by anything in
  # the VPC.
  client_security_group_ids = var.tool_executor.client_security_group_ids

  # Fixed count, no scaling policy. The executor holds no state between calls, so
  # scaling it is safe in principle; it is not scaled because nothing has measured
  # what the right signal is, and a target-tracking policy on CPU alone would
  # scale a socket-bound workload on the wrong number. `ecs-service` enforces the
  # consequence: with no metrics, both bounds must equal `desired_count`.
  autoscaling_metrics = []
  autoscaling_bounds = {
    min_capacity = var.tool_executor.desired_count
    max_capacity = var.tool_executor.desired_count
  }

  env                = var.tool_executor.env
  task_role_arn      = module.role["tool-executor"].role_arn
  execution_role_arn = var.tool_executor.execution_role_arn
  subnets            = var.private_subnet_ids
  log_group_name     = module.tool_executor_log_group.name
  region             = var.region

  vpc_id                               = var.vpc_id
  interface_endpoint_security_group_id = var.interface_endpoint_security_group_id
  gateway_endpoint_prefix_list_ids     = var.gateway_endpoint_prefix_list_ids

  tags = var.tags
}
