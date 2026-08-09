mock_provider "aws" {}

variables {
  plane                           = "dev"
  region                          = "eu-west-1"
  permissions_boundary_policy_arn = "arn:aws:iam::000000000000:policy/aex-dev-application-boundary"
  vpc_id                          = "vpc-0123456789abcdef0"
  private_subnet_ids              = ["subnet-0123456789abcdef0", "subnet-0123456789abcdef1"]
  public_subnet_ids               = ["subnet-0123456789abcdef2", "subnet-0123456789abcdef3"]
  service_security_group_ids      = ["sg-0123456789abcdef0"]
  alb_security_group_ids          = ["sg-0123456789abcdef1"]
  kms_key_arn                     = "arn:aws:kms:eu-west-1:000000000000:key/00000000-0000-4000-8000-000000000000"
  session_journal_stream_arn      = "arn:aws:dynamodb:eu-west-1:000000000000:table/aex-dev-euw1-session-journal/stream/2026-08-01T00:00:00.000"
  cluster_name                    = "aex-dev-euw1"

  # The `path_patterns` below are the part of the public surface that splits
  # cleanly, and only that part. `/api/telemetry/*`, `/api/sessions` and
  # everything under `/api/sessions/{sessionId}/` are deliberately absent:
  # `regional-stream` and `regional-session-api` both serve routes under
  # `/api/sessions/{sessionId}/`, discriminated by the segment *after* a
  # variable id, so no prefix split separates them. See this root's README.
  session_api = {
    name                              = "regional-session-api"
    image                             = "000000000000.dkr.ecr.eu-west-1.amazonaws.com/aex/regional-session-api@sha256:0000000000000000000000000000000000000000000000000000000000000000"
    cpu                               = 1024
    memory                            = 2048
    desired_count                     = 2
    stop_timeout                      = 30
    container_port                    = 8080
    health_check_grace_period_seconds = 60
    log_group_name                    = "/aex/dev/regional-session-api"
    log_retention_days                = 30
    execution_role_arn                = "arn:aws:iam::000000000000:role/aex-dev-ecs-execution"
    path_patterns                     = ["/api/workspace/*", "/api/operations/*", "/api/billing/*"]
    env = {
      AEX_PLANE  = "dev"
      AEX_REGION = "eu-west-1"
    }
    autoscaling_bounds = {
      min_capacity = 2
      max_capacity = 2
    }
    autoscaling_metrics = []
  }

  operation_queue = {
    name                      = "session-operation"
    visibility_timeout        = 300
    max_receive_count         = 5
    message_retention_seconds = 345600
    dlq_retention_seconds     = 1209600
  }

  stream_pipe = {
    name           = "aex-dev-session-journal-hint"
    filter_pattern = "{\"eventName\":[\"INSERT\",\"MODIFY\"]}"
    input_template = "{\"workId\": <$.dynamodb.NewImage.workId.S>}"
    batch_size     = 10
  }

  stream_service = {
    name                              = "regional-stream"
    image                             = "000000000000.dkr.ecr.eu-west-1.amazonaws.com/aex/regional-stream@sha256:0000000000000000000000000000000000000000000000000000000000000000"
    cpu                               = 1024
    memory                            = 2048
    desired_count                     = 2
    stop_timeout                      = 30
    container_port                    = 8080
    health_check_grace_period_seconds = 60
    log_group_name                    = "/aex/dev/regional-stream"
    log_retention_days                = 30
    execution_role_arn                = "arn:aws:iam::000000000000:role/aex-dev-ecs-execution"
    path_patterns                     = ["/api/events/*", "/api/logs/*", "/api/metrics/*", "/api/spans/*", "/api/traces/*"]
    env = {
      AEX_PLANE = "dev"
    }
    autoscaling_bounds = {
      min_capacity = 2
      max_capacity = 2
    }
    autoscaling_metrics = []
  }

  alb = {
    name               = "aex-dev-euw1-public"
    certificate_arn    = "arn:aws:acm:eu-west-1:000000000000:certificate/00000000-0000-4000-8000-000000000000"
    access_logs_bucket = "aex-dev-alb-logs-0a1b2c3d"
  }

  deployable_grants = {
    "regional-session-api" = {
      assume_principal = {
        type        = "Service"
        identifiers = ["ecs-tasks.amazonaws.com"]
      }
      wildcard_resource_allowlist = ["kms:GenerateRandom"]
      action_grants = [
        {
          sid              = "SessionJournal"
          actions          = ["dynamodb:GetItem", "dynamodb:PutItem", "dynamodb:TransactWriteItems"]
          resources        = ["arn:aws:dynamodb:eu-west-1:000000000000:table/aex-dev-euw1-session-journal"]
          scopable         = true
          condition_key    = "aws:ResourceTag/aex:plane"
          condition_values = ["dev"]
        },
      ]
    }

    "regional-stream" = {
      assume_principal = {
        type        = "Service"
        identifiers = ["ecs-tasks.amazonaws.com"]
      }
      wildcard_resource_allowlist = []
      action_grants = [
        {
          sid                = "ReadJournal"
          actions            = ["dynamodb:GetItem", "dynamodb:Query"]
          resources          = ["arn:aws:dynamodb:eu-west-1:000000000000:table/aex-dev-euw1-session-journal"]
          scopable           = true
          condition_operator = "ForAllValues:StringLike"
          condition_key      = "dynamodb:LeadingKeys"
          condition_values   = ["SESSION#*"]
        },
      ]
    }
  }
}

run "the_region_application_plans" {
  command = plan

  assert {
    condition     = length(module.role) == 2
    error_message = "One execution role must be created per deployable."
  }


  assert {
    condition     = jsondecode(module.role["regional-stream"].inline_policy_json).Statement[0].Condition["ForAllValues:StringLike"]["dynamodb:LeadingKeys"] == ["SESSION#*"]
    error_message = "The region wrapper must preserve reviewed set-qualified conditions."
  }
}

run "the_cluster_and_the_log_groups_are_created_by_this_root" {
  command = plan

  assert {
    condition     = module.cluster.name == var.cluster_name
    error_message = "The ECS cluster must be created under the configured name."
  }

  assert {
    condition     = module.stream_log_group.name == var.stream_service.log_group_name
    error_message = "The stream service log group must be created by this root, not assumed to exist."
  }

  assert {
    condition     = module.session_log_group.name == var.session_api.log_group_name
    error_message = "The session API log group must be created by this root; a Fargate service has no managed group waiting for it the way a Lambda did."
  }
}

run "each_service_drain_window_matches_its_own_target_group" {
  command = plan

  assert {
    condition     = module.stream_service.deregistration_delay == module.stream_target.deregistration_delay
    error_message = "The stream service and the target group in front of it must agree on the drain window."
  }

  assert {
    condition     = module.session_service.deregistration_delay == module.session_target.deregistration_delay
    error_message = "The session service and the target group in front of it must agree on the drain window."
  }

  assert {
    condition     = module.session_service.deregistration_delay >= 30
    error_message = "The service drain window must be at least the target group deregistration delay."
  }
}

run "both_request_path_services_attach_to_the_one_public_listener" {
  command = plan

  assert {
    condition     = var.stream_service.name != var.session_api.name
    error_message = "The two services are distinct deployables, each with its own target group and its own listener rule."
  }

  assert {
    condition = length(setintersection(
      toset(var.stream_service.path_patterns),
      toset(var.session_api.path_patterns),
    )) == 0
    error_message = "The two services must not claim the same path pattern."
  }

  assert {
    condition = alltrue([
      for p in concat(var.stream_service.path_patterns, var.session_api.path_patterns) :
      startswith(p, "/api/") && !startswith(p, "/internal")
    ])
    error_message = "Every forwarded pattern must be a public /api/ path; /internal/* stays unreachable from the public listener."
  }
}

run "the_session_api_runs_as_a_fargate_task_not_a_function" {
  command = plan

  assert {
    condition     = can(regex("@sha256:[0-9a-f]{64}$", var.session_api.image))
    error_message = "The session API image must be digest-pinned."
  }

  assert {
    condition = (
      var.session_api.cpu == 1024
      && var.session_api.memory == 2048
      && var.session_api.desired_count == 2
      && var.session_api.stop_timeout == 30
      && var.session_api.container_port == 8080
    )
    error_message = "The session API must carry the reviewed Fargate shape from release/units.toml: 1024/2048, two tasks, a 30 second stop timeout on port 8080."
  }

  assert {
    condition     = module.session_service.deregistration_delay >= 30
    error_message = "The session API must materialize as an ECS service behind a target group, with a drain window of at least 30 seconds."
  }

  assert {
    condition     = module.session_log_group.name == "/aex/${var.plane}/${var.session_api.name}"
    error_message = "The session API must log to its own plane-qualified group."
  }

  assert {
    condition = (
      contains(var.deployable_grants["regional-session-api"].assume_principal.identifiers, "ecs-tasks.amazonaws.com")
      && !contains(var.deployable_grants["regional-session-api"].assume_principal.identifiers, "lambda.amazonaws.com")
    )
    error_message = "A Fargate task assumes its role as ecs-tasks.amazonaws.com. The Lambda trust principal it used to carry would leave the task unable to assume the role at all, and the service would never start."
  }
}

run "the_stream_service_runs_a_digest_pinned_image" {
  command = plan

  assert {
    condition     = can(regex("@sha256:[0-9a-f]{64}$", var.stream_service.image))
    error_message = "The stream service image must be digest-pinned."
  }
}

run "the_stream_service_uses_reviewed_fixed_capacity_without_an_invented_metric" {
  command = plan

  assert {
    condition = (
      var.stream_service.desired_count == 2
      && var.stream_service.autoscaling_bounds.min_capacity == var.stream_service.desired_count
      && var.stream_service.autoscaling_bounds.max_capacity == var.stream_service.desired_count
      && length(var.stream_service.autoscaling_metrics) == 0
    )
    error_message = "The regional stream example must use two fixed tasks without inventing a custom autoscaling metric."
  }
}
