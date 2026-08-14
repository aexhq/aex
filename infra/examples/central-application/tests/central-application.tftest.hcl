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
  security_group_ids                   = ["sg-0123456789abcdef0"]
  interface_endpoint_security_group_id = "sg-0123456789abcdef1"
  gateway_endpoint_prefix_list_ids     = { s3 = "pl-0123456789abcdef0", dynamodb = "pl-0123456789abcdef1" }
  kms_key_arn                          = "arn:aws:kms:eu-west-1:000000000000:key/00000000-0000-4000-8000-000000000000"
  cluster_name                         = "aex-dev-central"
  artifact_bucket                      = "aex-infra-artifacts-dev-0a1b2c3d"

  alb = {
    name               = "aex-dev-euw1-control"
    certificate_arn    = "arn:aws:acm:eu-west-1:000000000000:certificate/00000000-0000-4000-8000-000000000000"
    access_logs_bucket = "aex-dev-euw1-alb-logs"
  }

  control_api = {
    name                              = "control-api"
    image                             = "000000000000.dkr.ecr.eu-west-1.amazonaws.com/aex/control-api@sha256:0000000000000000000000000000000000000000000000000000000000000000"
    cpu                               = 1024
    memory                            = 2048
    desired_count                     = 2
    stop_timeout                      = 30
    container_port                    = 8080
    health_check_grace_period_seconds = 60
    log_group_name                    = "/aex/dev/control-api"
    log_retention_days                = 30
    execution_role_arn                = "arn:aws:iam::000000000000:role/aex-dev-ecs-execution"
    env                               = { AEX_CENTRAL_API_PLANE = "dev", AEX_CENTRAL_API_PORT = "8080" }
    rules                             = [{ priority = 10, path_patterns = ["/api/auth/*", "/api/bootstrap", "/api/api-keys*", "/api/billing/*"] }]
    autoscaling_bounds                = { min_capacity = 2, max_capacity = 2 }
    autoscaling_metrics               = []
  }

  lambda_units = {
    "control-projection-worker" = {
      function_name           = "aex-dev-control-projection-worker"
      artifact_key            = "lambda/control-projection-worker/0000000000000000000000000000000000000000000000000000000000000000.zip"
      artifact_object_version = "projection-v1"
      artifact_sha256         = "47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU="
      runtime                 = "provided.al2023"
      handler                 = "bootstrap"
      memory_mb               = 512
      timeout_s               = 120
      reserved_concurrency    = 10
      log_retention_days      = 30
      env                     = { AEX_CONTROL_PROJECTION_PLANE = "dev" }
    }
    "stripe-webhook-edge" = {
      function_name               = "aex-dev-stripe-webhook-edge"
      artifact_key                = "lambda/stripe-webhook-edge/0000000000000000000000000000000000000000000000000000000000000000.zip"
      artifact_object_version     = "stripe-v1"
      artifact_sha256             = "47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU="
      runtime                     = "nodejs22.x"
      handler                     = "handler.js"
      memory_mb                   = 512
      timeout_s                   = 30
      reserved_concurrency        = 40
      log_retention_days          = 30
      env                         = { AEX_STRIPE_WEBHOOK_SECRET_ARN = "arn:aws:secretsmanager:eu-west-1:000000000000:secret:aex-dev-stripe" }
      public_function_url_enabled = true
    }
    "billing-worker" = {
      function_name           = "aex-dev-billing-worker"
      artifact_key            = "lambda/billing-worker/0000000000000000000000000000000000000000000000000000000000000000.zip"
      artifact_object_version = "billing-v1"
      artifact_sha256         = "47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU="
      runtime                 = "provided.al2023"
      handler                 = "bootstrap"
      memory_mb               = 1024
      timeout_s               = 300
      reserved_concurrency    = 40
      log_retention_days      = 30
      env                     = { AEX_BILLING_WORKER_PLANE = "dev" }
    }
  }

  event_sources = {
    projection = { unit = "control-projection-worker", source_arn = "arn:aws:sqs:eu-west-1:000000000000:aex-dev-control-projection", batch_size = 10 }
    billing    = { unit = "billing-worker", source_arn = "arn:aws:sqs:eu-west-1:000000000000:aex-dev-billing", batch_size = 10 }
  }

  database = {
    cluster_identifier        = "aex-dev-central"
    database_name             = "aex"
    master_username           = "aex_admin"
    engine_version            = "17.5"
    min_acu                   = 0.5
    max_acu                   = 8
    backup_retention_days     = 7
    final_snapshot_identifier = "aex-dev-central-final"
  }

  deployable_grants = {
    "control-api"               = { assume_principal = { type = "Service", identifiers = ["ecs-tasks.amazonaws.com"] }, wildcard_resource_allowlist = [], action_grants = [{ sid = "Data", actions = ["rds-data:ExecuteStatement"], resources = ["arn:aws:rds:eu-west-1:000000000000:cluster:aex-dev-central"], scopable = true, condition_key = "aws:ResourceTag/aex:plane", condition_values = ["dev"] }] }
    "control-projection-worker" = { assume_principal = { type = "Service", identifiers = ["lambda.amazonaws.com"] }, wildcard_resource_allowlist = [], action_grants = [{ sid = "Projection", actions = ["dynamodb:PutItem"], resources = ["arn:aws:dynamodb:eu-west-1:000000000000:table/aex-dev-regional-authz-projection"], scopable = true, condition_key = "aws:ResourceTag/aex:plane", condition_values = ["dev"] }] }
    "stripe-webhook-edge"       = { assume_principal = { type = "Service", identifiers = ["lambda.amazonaws.com"] }, wildcard_resource_allowlist = [], action_grants = [{ sid = "Secret", actions = ["secretsmanager:GetSecretValue"], resources = ["arn:aws:secretsmanager:eu-west-1:000000000000:secret:aex-dev-stripe"], scopable = true, condition_key = "aws:ResourceTag/aex:plane", condition_values = ["dev"] }] }
    "billing-worker"            = { assume_principal = { type = "Service", identifiers = ["lambda.amazonaws.com"] }, wildcard_resource_allowlist = [], action_grants = [{ sid = "Ledger", actions = ["rds-data:ExecuteStatement"], resources = ["arn:aws:rds:eu-west-1:000000000000:cluster:aex-dev-central"], scopable = true, condition_key = "aws:ResourceTag/aex:plane", condition_values = ["dev"] }] }
  }
}

run "exact_central_session_mvp_topology" {
  command = plan

  assert {
    condition     = local.central_compute_units == toset(["control-api", "control-projection-worker", "stripe-webhook-edge", "billing-worker"])
    error_message = "Central Terraform must place exactly the four central session-MVP units."
  }

  assert {
    condition = (
      var.lambda_units["stripe-webhook-edge"].public_function_url_enabled
      && alltrue([
        for name, unit in var.lambda_units :
        name == "stripe-webhook-edge" || !unit.public_function_url_enabled
      ])
    )
    error_message = "Only the verified Stripe webhook edge needs direct public Lambda ingress."
  }

  assert {
    condition     = can(regex("@sha256:[0-9a-f]{64}$", var.control_api.image))
    error_message = "The control API must run a digest-pinned image."
  }
}

run "removed_central_units_are_unrepresentable" {
  command = plan

  assert {
    condition = length(setintersection(local.central_compute_units, toset([
      "central-identity-api", "central-control-api", "finance-api", "finance-settlement-worker",
      "finance-reconcile", "provider-cost-reconciler", "usage-receipt-dispatcher", "stripe-command-edge"
    ]))) == 0
    error_message = "No retired central unit may survive through a generic map."
  }
}
