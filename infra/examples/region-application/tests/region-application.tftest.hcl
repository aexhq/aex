mock_provider "aws" {}

variables {
  plane                           = "dev"
  region                          = "eu-west-1"
  permissions_boundary_policy_arn = "arn:aws:iam::000000000000:policy/aex-dev-application-boundary"
  vpc_id                          = "vpc-0123456789abcdef0"
  private_subnet_ids              = ["subnet-0123456789abcdef0", "subnet-0123456789abcdef1"]
  public_subnet_ids               = ["subnet-0123456789abcdef2", "subnet-0123456789abcdef3"]
  kms_key_arn                     = "arn:aws:kms:eu-west-1:000000000000:key/00000000-0000-4000-8000-000000000000"
  session_journal_stream_arn      = "arn:aws:dynamodb:eu-west-1:000000000000:table/aex-dev-euw1-session-journal/stream/2026-08-01T00:00:00.000"
  cluster_name                    = "aex-dev-euw1"

  # No group ids: every group is created by the module that owns the resource
  # it protects. What the foundation still has to hand over is what the task
  # groups are allowed to reach.
  interface_endpoint_security_group_id = "sg-0123456789abcdef0"

  gateway_endpoint_prefix_list_ids = {
    s3       = "pl-0123456789abcdef0"
    dynamodb = "pl-0123456789abcdef1"
  }

  # Every regional first segment now resolves to exactly one serving artifact,
  # workspace-scoped routes included: `/api/sessions/`, `/api/workspace/`,
  # `/api/operations/` and `/api/billing/` are `regional-session-api`'s,
  # `/api/streams/` is `regional-stream`'s. No prefix needs priority precedence
  # to route correctly, and both services claim theirs by prefix below.
  #
  # `/api/observations/*`, `/api/secrets/*` and `/api/otlp/*` are the other
  # three artifacts' prefixes and are absent here because this root deploys two
  # services and none of those three is one of them. See the README.
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
    # `/api/sessions` and `/api/sessions/*` are two condition values because an
    # ALB pattern ending in `/*` does not match the bare collection: `POST
    # /api/sessions` and `GET /api/sessions` carry no session id.
    rules = [
      {
        priority      = 20
        path_patterns = ["/api/workspace/*", "/api/operations/*", "/api/billing/*"]
      },
      {
        priority      = 21
        path_patterns = ["/api/sessions", "/api/sessions/*"]
      },
    ]
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
    # All twenty-four NDJSON routes sit under `/api/streams/` now — the
    # session-scoped ones and the workspace-scoped ones alike — so the whole
    # surface is one condition value in one rule. It used to be seven top-level
    # prefixes across two rules, which the per-rule quota of five condition
    # values forced; the quota has not moved, the surface has.
    rules = [
      {
        priority      = 10
        path_patterns = ["/api/streams/*"]
      },
    ]
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
      toset(flatten([for r in var.stream_service.rules : r.path_patterns])),
      toset(flatten([for r in var.session_api.rules : r.path_patterns])),
    )) == 0
    error_message = "The two services must not claim the same path pattern."
  }

  assert {
    condition = alltrue([
      for p in flatten([
        for r in concat(var.stream_service.rules, var.session_api.rules) : r.path_patterns
      ]) :
      startswith(p, "/api/") && !startswith(p, "/internal")
    ])
    error_message = "Every forwarded pattern must be a public /api/ path; /internal/* stays unreachable from the public listener."
  }

  assert {
    condition = length(setintersection(
      toset(module.stream_target.rule_priorities),
      toset(module.session_target.rule_priorities),
    )) == 0
    error_message = "The two services must occupy disjoint listener priorities. Each module instantiation proves its own priorities unique; only this root can see both."
  }

  # The stream service's whole surface is one prefix, so one rule carries it.
  # The per-rule quota still bounds a rule rather than a service, which the
  # session API's two rules and the module's own tests exercise.
  assert {
    condition     = length(module.stream_target.rule_priorities) == 1
    error_message = "The stream service's whole public surface is `/api/streams/*`: one condition value, and therefore one rule."
  }

  # The property the API surface now supplies and this root depends on: every
  # forwarded pattern is anchored on its first path segment, and no first
  # segment is claimed by both services. A prefix match is therefore sufficient
  # to route, and correctness does not rest on rule precedence. This could not
  # hold while `/api/sessions/{sessionId}/` was served by three artifacts at
  # once, nor while `/api/logs/query` and `/api/logs/stream` went to different
  # ones: in both cases only a segment after the shared prefix told them apart.
  assert {
    condition = length(setintersection(
      toset([for p in flatten([for r in var.stream_service.rules : r.path_patterns]) : split("/", p)[2]]),
      toset([for p in flatten([for r in var.session_api.rules : r.path_patterns]) : split("/", p)[2]]),
    )) == 0
    error_message = "The two services must not share a first path segment. If they do, no prefix separates them and routing falls back to rule precedence."
  }

  # No pattern may wildcard the first segment, or the anchoring above is vacuous.
  assert {
    condition = alltrue([
      for p in flatten([
        for r in concat(var.stream_service.rules, var.session_api.rules) : r.path_patterns
      ]) :
      !strcontains(split("/", p)[2], "*") && !strcontains(split("/", p)[2], "?")
    ])
    error_message = "A forwarded pattern must name its first path segment literally; an ALB `*` matches across `/`, so a wildcard there would swallow another service's prefix."
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
