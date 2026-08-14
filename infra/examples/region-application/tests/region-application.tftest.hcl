mock_provider "aws" {
  mock_data "aws_s3_object" {
    defaults = { checksum_sha256 = "47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=" }
  }
}

variables {
  plane                                = "dev"
  region                               = "eu-west-1"
  permissions_boundary_policy_arn      = "arn:aws:iam::000000000000:policy/aex-dev-boundary"
  vpc_id                               = "vpc-0123456789abcdef0"
  private_subnet_ids                   = ["subnet-0123456789abcdef0", "subnet-0123456789abcdef1"]
  public_subnet_ids                    = ["subnet-0123456789abcdef2", "subnet-0123456789abcdef3"]
  interface_endpoint_security_group_id = "sg-0123456789abcdef0"
  gateway_endpoint_prefix_list_ids     = { s3 = "pl-0123456789abcdef0", dynamodb = "pl-0123456789abcdef1" }
  kms_key_arn                          = "arn:aws:kms:eu-west-1:000000000000:key/00000000-0000-4000-8000-000000000000"
  cluster_name                         = "aex-dev-eu-west-1"
  service_discovery_namespace          = "aex-dev.internal"
  artifact_bucket                      = "aex-infra-artifacts-dev-0a1b2c3d"
  alb = {
    name               = "aex-dev-euw1-session"
    certificate_arn    = "arn:aws:acm:eu-west-1:000000000000:certificate/00000000-0000-4000-8000-000000000000"
    access_logs_bucket = "aex-dev-euw1-alb-logs"
  }
  session_rules = [
    { priority = 10, path_patterns = ["/api/sessions", "/api/sessions/*"] },
    { priority = 20, path_patterns = ["/api/files", "/api/files/*", "/api/uploads", "/api/uploads/*"] },
  ]

  services = {
    "session-api" = {
      image                             = "000000000000.dkr.ecr.eu-west-1.amazonaws.com/aex/session-api@sha256:0000000000000000000000000000000000000000000000000000000000000000"
      cpu                               = 1024
      memory                            = 2048
      desired_count                     = 2
      stop_timeout                      = 30
      container_port                    = 8080
      health_check_grace_period_seconds = 60
      log_group_name                    = "/aex/dev/session-api"
      log_retention_days                = 30
      execution_role_arn                = "arn:aws:iam::000000000000:role/aex-dev-ecs-execution"
      env                               = { AEX_SESSION_API_PLANE = "dev" }
      autoscaling_bounds                = { min_capacity = 2, max_capacity = 2 }
      autoscaling_metrics               = []
    }
    "brain-mux" = {
      image               = "000000000000.dkr.ecr.eu-west-1.amazonaws.com/aex/brain-mux@sha256:0000000000000000000000000000000000000000000000000000000000000000"
      cpu                 = 2048
      memory              = 4096
      desired_count       = 1
      stop_timeout        = 120
      container_port      = 8080
      log_group_name      = "/aex/dev/brain-mux"
      log_retention_days  = 30
      execution_role_arn  = "arn:aws:iam::000000000000:role/aex-dev-ecs-execution"
      env                 = { AEX_BRAIN_MUX_PLANE = "dev", AEX_MAX_ACTIVE_ACTIVATIONS = "16" }
      autoscaling_bounds  = { min_capacity = 1, max_capacity = 1 }
      autoscaling_metrics = []
    }
    "tool-mux" = {
      image               = "000000000000.dkr.ecr.eu-west-1.amazonaws.com/aex/tool-mux@sha256:0000000000000000000000000000000000000000000000000000000000000000"
      cpu                 = 1024
      memory              = 2048
      desired_count       = 2
      stop_timeout        = 120
      container_port      = 8080
      log_group_name      = "/aex/dev/tool-mux"
      log_retention_days  = 30
      execution_role_arn  = "arn:aws:iam::000000000000:role/aex-dev-ecs-execution"
      env                 = { AEX_TOOL_MUX_PLANE = "dev" }
      autoscaling_bounds  = { min_capacity = 2, max_capacity = 2 }
      autoscaling_metrics = []
    }
  }

  lambda_units = {
    "file-ingest-worker" = {
      function_name           = "aex-dev-file-ingest-worker"
      artifact_key            = "lambda/file-ingest-worker/0000000000000000000000000000000000000000000000000000000000000000.zip"
      artifact_object_version = "file-v1"
      artifact_sha256         = "47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU="
      memory_mb               = 1024
      timeout_s               = 300
      reserved_concurrency    = 20
      log_retention_days      = 30
      env                     = { AEX_FILE_INGEST_PLANE = "dev" }
    }
    "runtime-control-worker" = {
      function_name           = "aex-dev-runtime-control-worker"
      artifact_key            = "lambda/runtime-control-worker/0000000000000000000000000000000000000000000000000000000000000000.zip"
      artifact_object_version = "runtime-v1"
      artifact_sha256         = "47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU="
      memory_mb               = 1024
      timeout_s               = 120
      reserved_concurrency    = 20
      log_retention_days      = 30
      env                     = { AEX_RUNTIME_CONTROL_PLANE = "dev" }
    }
    "session-maintenance-worker" = {
      function_name           = "aex-dev-session-maintenance-worker"
      artifact_key            = "lambda/session-maintenance-worker/0000000000000000000000000000000000000000000000000000000000000000.zip"
      artifact_object_version = "maintenance-v1"
      artifact_sha256         = "47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU="
      memory_mb               = 1024
      timeout_s               = 300
      reserved_concurrency    = 20
      log_retention_days      = 30
      env                     = { AEX_SESSION_MAINTENANCE_PLANE = "dev" }
    }
  }

  event_sources = {
    file        = { unit = "file-ingest-worker", source_arn = "arn:aws:dynamodb:eu-west-1:000000000000:table/aex-dev-regional-file/stream/2026-08-14T00:00:00.000", batch_size = 10, starting_position = "LATEST" }
    runtime     = { unit = "runtime-control-worker", source_arn = "arn:aws:dynamodb:eu-west-1:000000000000:table/aex-dev-runtime-activity/stream/2026-08-14T00:00:00.000", batch_size = 10, starting_position = "LATEST" }
    maintenance = { unit = "session-maintenance-worker", source_arn = "arn:aws:dynamodb:eu-west-1:000000000000:table/aex-dev-session-authority/stream/2026-08-14T00:00:00.000", batch_size = 10, starting_position = "LATEST" }
  }

  deployable_grants = {
    "session-api"                = { assume_principal = { type = "Service", identifiers = ["ecs-tasks.amazonaws.com"] }, wildcard_resource_allowlist = [], action_grants = [{ sid = "SessionRead", actions = ["dynamodb:GetItem"], resources = ["arn:aws:dynamodb:eu-west-1:000000000000:table/aex-dev-session-authority"], scopable = true, condition_key = "aws:ResourceTag/aex:plane", condition_values = ["dev"] }] }
    "brain-mux"                  = { assume_principal = { type = "Service", identifiers = ["ecs-tasks.amazonaws.com"] }, wildcard_resource_allowlist = [], action_grants = [{ sid = "BrainWork", actions = ["dynamodb:TransactWriteItems"], resources = ["arn:aws:dynamodb:eu-west-1:000000000000:table/aex-dev-regional-work"], scopable = true, condition_key = "aws:ResourceTag/aex:plane", condition_values = ["dev"] }] }
    "tool-mux"                   = { assume_principal = { type = "Service", identifiers = ["ecs-tasks.amazonaws.com"] }, wildcard_resource_allowlist = [], action_grants = [{ sid = "ToolRuntime", actions = ["dynamodb:GetItem"], resources = ["arn:aws:dynamodb:eu-west-1:000000000000:table/aex-dev-runtime-activity"], scopable = true, condition_key = "aws:ResourceTag/aex:plane", condition_values = ["dev"] }] }
    "file-ingest-worker"         = { assume_principal = { type = "Service", identifiers = ["lambda.amazonaws.com"] }, wildcard_resource_allowlist = [], action_grants = [{ sid = "FileObjects", actions = ["s3:PutObject"], resources = ["arn:aws:s3:::aex-dev-workspace-files/*"], scopable = true, condition_key = "aws:ResourceTag/aex:plane", condition_values = ["dev"] }] }
    "runtime-control-worker"     = { assume_principal = { type = "Service", identifiers = ["lambda.amazonaws.com"] }, wildcard_resource_allowlist = [], action_grants = [{ sid = "RuntimeActivity", actions = ["dynamodb:UpdateItem"], resources = ["arn:aws:dynamodb:eu-west-1:000000000000:table/aex-dev-runtime-activity"], scopable = true, condition_key = "aws:ResourceTag/aex:plane", condition_values = ["dev"] }] }
    "session-maintenance-worker" = { assume_principal = { type = "Service", identifiers = ["lambda.amazonaws.com"] }, wildcard_resource_allowlist = [], action_grants = [{ sid = "SessionCleanup", actions = ["dynamodb:DeleteItem"], resources = ["arn:aws:dynamodb:eu-west-1:000000000000:table/aex-dev-session-authority"], scopable = true, condition_key = "aws:ResourceTag/aex:plane", condition_values = ["dev"] }] }
  }
}

run "exact_regional_session_mvp_topology" {
  command = plan

  assert {
    condition     = local.regional_compute_units == toset(["session-api", "file-ingest-worker", "brain-mux", "tool-mux", "runtime-control-worker", "session-maintenance-worker"])
    error_message = "Regional Terraform must place exactly the six regional/runtime session-MVP units."
  }

  assert {
    condition = (
      !module.session_api.public_https_egress_enabled
      && module.brain_mux.public_https_egress_enabled
      && !module.tool_mux.public_https_egress_enabled
    )
    error_message = "Only Brain may reach public provider endpoints; session-api and Tool Mux must have no public egress because MCP runs inside Hands."
  }
}

run "mux_network_and_iam_boundaries" {
  command = plan

  assert {
    condition = (
      !strcontains(module.role["brain-mux"].inline_policy_json, "lambdamicrovms")
      && !strcontains(module.role["brain-mux"].inline_policy_json, "secretsmanager")
      && !strcontains(module.role["tool-mux"].inline_policy_json, "rds-data")
      && !strcontains(module.role["file-ingest-worker"].inline_policy_json, "secretsmanager")
    )
    error_message = "Brain cannot reach Hands/MCP secrets, Tool Mux cannot mutate money, and file ingest cannot read session secrets."
  }
}

run "session_maintenance_has_no_automatic_content_storage" {
  command = plan

  assert {
    condition = (
      !strcontains(module.role["session-maintenance-worker"].inline_policy_json, "session-content/v1/*")
      && !strcontains(module.role["file-ingest-worker"].inline_policy_json, "s3:DeleteObject")
    )
    error_message = "Session maintenance must not acquire automatic customer-content authority."
  }
}
