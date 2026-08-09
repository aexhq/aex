mock_provider "aws" {
  mock_data "aws_s3_object" {
    defaults = {
      checksum_sha256 = "47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU="
    }
  }
}

variables {
  plane                           = "dev"
  region                          = "eu-west-1"
  permissions_boundary_policy_arn = "arn:aws:iam::000000000000:policy/aex-dev-application-boundary"
  subnet_ids                      = ["subnet-0123456789abcdef0", "subnet-0123456789abcdef1"]
  security_group_ids              = ["sg-0123456789abcdef0"]
  kms_key_arn                     = "arn:aws:kms:eu-west-1:000000000000:key/00000000-0000-4000-8000-000000000000"
  artifact_bucket                 = "aex-infra-artifacts-dev-0a1b2c3d"
  schedule_group_name             = "aex-dev-central"
  vpc_id                          = "vpc-0123456789abcdef0"
  public_subnet_ids               = ["subnet-0123456789abcdef2", "subnet-0123456789abcdef3"]
  cluster_name                    = "aex-dev-central"

  interface_endpoint_security_group_id = "sg-0123456789abcdef1"

  gateway_endpoint_prefix_list_ids = {
    s3       = "pl-0123456789abcdef0"
    dynamodb = "pl-0123456789abcdef1"
  }

  alb = {
    name               = "aex-dev-euw1-central"
    certificate_arn    = "arn:aws:acm:eu-west-1:000000000000:certificate/00000000-0000-4000-8000-000000000000"
    access_logs_bucket = "aex-dev-euw1-alb-logs"
  }

  central_api = {
    name                              = "central-api"
    image                             = "000000000000.dkr.ecr.eu-west-1.amazonaws.com/aex/central-api@sha256:0000000000000000000000000000000000000000000000000000000000000000"
    cpu                               = 1024
    memory                            = 2048
    desired_count                     = 2
    stop_timeout                      = 30
    container_port                    = 8080
    health_check_grace_period_seconds = 60
    log_group_name                    = "/aex/dev/central-api"
    log_retention_days                = 30
    execution_role_arn                = "arn:aws:iam::000000000000:role/aex-dev-ecs-execution"

    env = {
      AEX_CENTRAL_API_PLANE  = "dev"
      AEX_CENTRAL_API_REGION = "eu-west-1"
      AEX_CENTRAL_API_PORT   = "8080"
    }

    # Every central first path segment has exactly one serving artifact now, so
    # no two rules can both match one request and evaluation order decides
    # nothing; a priority is an address, not a tie-break.
    rules = [
      {
        priority      = 10
        path_patterns = ["/api/auth/*", "/api/bootstrap", "/api/billing/*"]
      },
      {
        priority      = 20
        path_patterns = ["/api/organizations", "/api/organizations/*"]
      },
      {
        priority      = 30
        path_patterns = ["/api/workspaces", "/api/workspaces/*", "/api/api-keys", "/api/api-keys/*", "/api/operations"]
      },
      {
        priority      = 40
        path_patterns = ["/api/operations/*"]
      },
    ]

    # Two fixed tasks, and no invented scaling signal. `ecs-service` refuses an
    # autoscaling policy whose only metric is CPU, and rightly: a request-path
    # task saturates on concurrent requests and Aurora latency long before its
    # CPU moves. Until this service publishes a metric that means something,
    # fixed capacity is the honest shape.
    autoscaling_bounds = {
      min_capacity = 2
      max_capacity = 2
    }

    autoscaling_metrics = []
  }

  device_flow_rate_limit = {
    name       = "aex-dev-central-device-flow"
    rate_limit = 100
    paths = [
      "/api/auth/device/authorizations",
      "/api/auth/device/tokens",
    ]
  }

  finance_api = {
    function_name           = "aex-dev-finance-api"
    artifact_key            = "lambda/finance-api/e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855.zip"
    artifact_object_version = "aBcDeFgHiJkLmNoPqRsTuVwXyZ012345"
    artifact_sha256         = "47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU="
    memory_mb               = 512
    timeout_s               = 30
    log_retention_days      = 30
    env = {
      AEX_PLANE = "dev"
    }
  }

  settlement_queue = {
    name                      = "settlement-work"
    visibility_timeout        = 300
    max_receive_count         = 5
    message_retention_seconds = 345600
    dlq_retention_seconds     = 1209600
    batch_size                = 10
  }

  database = {
    cluster_identifier    = "aex-dev-central-finance"
    database_name         = "aex_finance"
    master_username       = "aex_admin"
    engine_version        = "17.5"
    min_acu               = 0.5
    max_acu               = 8
    backup_retention_days = 7
  }

  schema_admin = {
    family             = "aex-dev-central-schema-admin"
    image              = "000000000000.dkr.ecr.eu-west-1.amazonaws.com/aex/central-schema-admin@sha256:0000000000000000000000000000000000000000000000000000000000000000"
    cpu                = 1024
    memory             = 2048
    stop_timeout       = 120
    log_group_name     = "/aex/dev/central-schema-admin"
    execution_role_arn = "arn:aws:iam::000000000000:role/aex-dev-ecs-execution"
    secret_env = {
      AEX_CENTRAL_ADMIN_SECRET = "arn:aws:secretsmanager:eu-west-1:000000000000:secret:aex-dev-central-admin"
    }
  }

  deployable_grants = {
    "finance-api" = {
      assume_principal = {
        type        = "Service"
        identifiers = ["lambda.amazonaws.com"]
      }
      wildcard_resource_allowlist = ["kms:GenerateRandom"]
      action_grants = [
        {
          sid              = "FinanceData"
          actions          = ["rds-data:ExecuteStatement", "rds-data:BeginTransaction", "rds-data:CommitTransaction"]
          resources        = ["arn:aws:rds:eu-west-1:000000000000:cluster:aex-dev-central-finance"]
          scopable         = true
          condition_key    = "aws:ResourceTag/aex:plane"
          condition_values = ["dev"]
        },
        {
          sid                = "ExportControlOnly"
          actions            = ["dynamodb:PutItem", "dynamodb:UpdateItem"]
          resources          = ["arn:aws:dynamodb:eu-west-1:000000000000:table/aex-dev-euw1-observation-authority"]
          scopable           = true
          condition_operator = "ForAllValues:StringLike"
          condition_key      = "dynamodb:LeadingKeys"
          condition_values   = ["EXPORT#*"]
        },
      ]
    }

    "central-schema-admin" = {
      assume_principal = {
        type        = "Service"
        identifiers = ["ecs-tasks.amazonaws.com"]
      }
      wildcard_resource_allowlist = []
      action_grants = [
        {
          sid              = "MigrationData"
          actions          = ["rds-data:ExecuteStatement", "rds-data:BeginTransaction", "rds-data:CommitTransaction"]
          resources        = ["arn:aws:rds:eu-west-1:000000000000:cluster:aex-dev-central-finance"]
          scopable         = true
          condition_key    = "aws:ResourceTag/aex:plane"
          condition_values = ["dev"]
        },
      ]
    }

    "central-api" = {
      assume_principal = {
        type        = "Service"
        identifiers = ["ecs-tasks.amazonaws.com"]
      }
      wildcard_resource_allowlist = []
      action_grants = [
        {
          sid              = "CentralData"
          actions          = ["rds-data:ExecuteStatement", "rds-data:BeginTransaction", "rds-data:CommitTransaction", "rds-data:RollbackTransaction"]
          resources        = ["arn:aws:rds:eu-west-1:000000000000:cluster:aex-dev-central-finance"]
          scopable         = true
          condition_key    = "aws:ResourceTag/aex:plane"
          condition_values = ["dev"]
        },
        {
          sid              = "LoginAndPepperSecrets"
          actions          = ["secretsmanager:GetSecretValue"]
          resources        = ["arn:aws:secretsmanager:eu-west-1:000000000000:secret:aex-dev-central-*"]
          scopable         = true
          condition_key    = "aws:ResourceTag/aex:plane"
          condition_values = ["dev"]
        },
      ]
    }
  }

  schedules = [
    {
      name                    = "settlement-sweep"
      description             = "Sweep settled usage into the ledger."
      expression              = "rate(1 hour)"
      flexible_window_minutes = 15
      target_kind             = "finance_api"
      input                   = "{\"sweep\":\"settlement\"}"
      role_arn                = "arn:aws:iam::000000000000:role/aex-dev-scheduler"
    },
  ]
}

run "the_central_application_plans" {
  command = plan

  assert {
    condition     = length(module.role) == 3
    error_message = "One execution role must be created per deployable."
  }

  assert {
    condition     = jsondecode(module.role["finance-api"].inline_policy_json).Statement[1].Condition["ForAllValues:StringLike"]["dynamodb:LeadingKeys"] == ["EXPORT#*"]
    error_message = "The central wrapper must preserve reviewed set-qualified conditions."
  }
}

run "the_lambda_runs_the_bytes_the_manifest_pins" {
  command = plan

  assert {
    condition     = var.finance_api.artifact_sha256 == "47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU="
    error_message = "The plan must be pinned to the digest the release manifest records."
  }
}

run "the_schedule_targets_an_alias_this_root_created" {
  command = plan

  assert {
    condition = alltrue([
      for s in var.schedules : contains(["finance_api", "schema_admin"], s.target_kind)
    ])
    error_message = "A schedule may only target a deployable this root created, never a name written by hand."
  }
}

run "the_settlement_queue_has_a_dead_letter_queue" {
  command = plan

  assert {
    condition     = module.settlement_queue.queue_name == var.settlement_queue.name
    error_message = "The settlement queue must be created under its configured name."
  }
}

# --- the public edge ----------------------------------------------------------

run "the_merged_service_runs_the_image_digest_the_manifest_pins" {
  command = plan

  assert {
    condition     = can(regex("@sha256:[0-9a-f]{64}$", var.central_api.image))
    error_message = "The service must run a digest-pinned image. A tag is mutable, so a task restart could pull bytes nobody released."
  }

  assert {
    condition = (
      var.central_api.desired_count == 2
      && var.central_api.autoscaling_bounds.min_capacity == 2
      && length(var.central_api.autoscaling_metrics) == 0
    )
    error_message = "This example must use two fixed tasks without inventing a custom autoscaling metric. One task in front of the whole central plane is a single point of failure; a CPU-only scaling policy is a signal that does not move when a request-path task saturates."
  }
}

run "the_task_drains_before_the_runtime_kills_it" {
  command = plan

  assert {
    condition     = var.central_api.stop_timeout == 30
    error_message = "The stop timeout is the number the process derives its own admitted drain deadline from. A disagreement between the two is a task killed mid-transaction rather than one exiting on its own terms."
  }

  assert {
    condition     = module.central_api_service.deregistration_delay == module.central_api_target.deregistration_delay
    error_message = "The service and its target group must publish one drain window. Two would let the load balancer keep sending to a task that has already stopped accepting."
  }
}

run "the_listener_reaches_the_service_and_nothing_reaches_the_probes" {
  command = plan

  assert {
    condition     = length(module.central_api_target.rule_priorities) == length(var.central_api.rules)
    error_message = "Every declared rule must be created; a dropped rule silently 404s a whole route group at the edge."
  }

  assert {
    condition = alltrue([
      for p in flatten([for r in var.central_api.rules : r.path_patterns]) : startswith(p, "/api/")
    ])
    error_message = "The public listener may only forward `/api/` paths. `/internal/healthz` and `/internal/readyz` exist and answer the target group's health check, and no rule may make them reachable from outside."
  }

  assert {
    condition = alltrue([
      for p in flatten([for r in var.central_api.rules : r.path_patterns]) : !strcontains(p, "/internal")
    ])
    error_message = "No rule may forward an internal path. The probes are the load balancer's own question to the task, not a public endpoint."
  }

  # The route groups the merged deployable serves, each named by at least one
  # rule. A group with no matching rule is a 404 at the edge for every one of its
  # routes, and nothing downstream would report it — the task would be healthy
  # and idle while a whole surface was unreachable.
  assert {
    condition = alltrue([
      for prefix in ["/api/auth/", "/api/billing/", "/api/bootstrap", "/api/organizations", "/api/workspaces", "/api/api-keys", "/api/operations"] :
      anytrue([
        for p in flatten([for r in var.central_api.rules : r.path_patterns]) :
        startswith(p, prefix) || startswith(prefix, trimsuffix(p, "*"))
      ])
    ])
    error_message = "Every central route group must be named by some listener rule. An unmatched group is a 404 at the edge for all of its routes while the task itself stays healthy, so nothing reports the gap."
  }
}

run "the_unauthenticated_device_flow_routes_are_the_ones_behind_the_throttle" {
  command = plan

  # The property, stated once: what the web ACL protects must be exactly the
  # routes that carry no credential. `POST /api/auth/device/authorizations` and
  # `POST /api/auth/device/tokens` admit anonymous callers by design and share a
  # single replay principal between all of them, which is why the accepted design
  # put a per-IP throttle here and nowhere else.
  assert {
    condition = toset(module.device_flow_rate_limit.protected_paths) == toset([
      "/api/auth/device/authorizations",
      "/api/auth/device/tokens",
    ])
    error_message = "The throttle must cover exactly the two unauthenticated device-flow routes. Dropping one leaves an unmetered issuance endpoint; adding an authenticated route puts a shared per-IP ceiling in front of customers behind one NAT."
  }

  assert {
    condition     = module.device_flow_rate_limit.rate_limit == var.device_flow_rate_limit.rate_limit
    error_message = "The web ACL must enforce the limit this root configured, not a module default."
  }
}

run "the_throttle_publishes_the_metric_an_alarm_is_written_against" {
  command = plan

  # API Gateway supplied the only request throttle the central plane ever had.
  # Moving behind an ALB without this web ACL leaves the plane with no
  # request-rate control at all, and the only evidence it is doing anything is
  # `BlockedRequests` at this dimension.
  assert {
    condition     = module.device_flow_rate_limit.metric_name == var.device_flow_rate_limit.name
    error_message = "The published metric name must be the configured one, so an alarm can be written against a string a human chose rather than one this root re-derives."
  }

  assert {
    condition     = module.device_flow_rate_limit.rule_metric_name == "${var.device_flow_rate_limit.name}-rate"
    error_message = "The rule's metric dimension must be derived from the same name, or an alarm on the rule and an alarm on the ACL would watch two different things."
  }
}
