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

  kms_key_arn_by_authority = { for k, m in module.authority_key : k => m.key_arn }

  tags = var.tags
}

module "content" {
  source = "../../modules/content-bucket"

  plane              = var.plane
  region             = var.region
  purpose            = var.content_bucket_purpose
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

  deployable                  = "session-stream-api"
  plane                       = var.plane
  region                      = var.region
  assume_principal            = var.session_api_grants.assume_principal
  action_grants               = var.session_api_grants.action_grants
  wildcard_resource_allowlist = var.session_api_grants.wildcard_resource_allowlist
  boundary_policy_arn         = var.permissions_boundary_policy_arn
  tags                        = var.tags
}

module "cluster" {
  source = "../../modules/ecs-cluster"

  name = var.cluster_name
  tags = var.tags
}

module "session_log_group" {
  source = "../../modules/log-group"

  name           = var.session_api.log_group_name
  retention_days = var.session_api.log_retention_days
  kms_key_arn    = module.authority_key[var.log_authority].key_arn
  tags           = var.tags
}

# `session-stream-api` is a Fargate service, not a function. This root stands
# it up with no public edge, exactly as it stood up the function with no API
# gateway: a self-hoster puts their own ingress in front of it. `ecs-service`
# rejects a health check grace period without a target group, which is why none
# is set here.
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

  autoscaling_bounds  = var.session_api.autoscaling_bounds
  autoscaling_metrics = var.session_api.autoscaling_metrics

  env                = var.session_api.env
  task_role_arn      = module.session_api_role.role_arn
  execution_role_arn = var.session_api.execution_role_arn
  subnets            = module.network.subnet_ids.private
  log_group_name     = module.session_log_group.name
  region             = var.region

  # The service creates its own task group. With no edge in front of it the
  # group admits nothing at all, and its egress is TLS to the endpoints this
  # network already stands up - which is the whole of what the task can reach,
  # because there is no NAT gateway and no route to the internet.
  vpc_id                               = module.network.vpc_id
  interface_endpoint_security_group_id = module.network.interface_endpoint_security_group_id
  gateway_endpoint_prefix_list_ids     = module.network.gateway_endpoint_prefix_list_ids

  tags = var.tags
}
