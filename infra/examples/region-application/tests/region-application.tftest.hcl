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
  internal_namespace              = "aex-dev.internal"

  # No group ids: every group is created by the module that owns the resource
  # it protects. What the foundation still has to hand over is what the task
  # groups are allowed to reach.
  interface_endpoint_security_group_id = "sg-0123456789abcdef0"

  gateway_endpoint_prefix_list_ids = {
    s3       = "pl-0123456789abcdef0"
    dynamodb = "pl-0123456789abcdef1"
  }

  # Every regional first segment resolves to exactly one serving artifact,
  # workspace-scoped routes included: `/api/sessions/`, `/api/workspace/`,
  # `/api/operations/`, `/api/billing/` and `/api/streams/`. No prefix needs
  # priority precedence to route correctly.
  #
  # `regional-session-api` and `regional-stream` merged into `session-stream-api`,
  # so the two rule bands are now one service's. The prefixes stay disjoint in the
  # contract, which is what would let a future root split them back across two
  # target groups without touching a route.
  #
  # `/api/observations/*`, `/api/secrets/*` and `/api/otlp/*` are the other three
  # artifacts' prefixes and are absent here because this root deploys one service
  # and none of those three is it. See the README.
  session_stream_api = {
    name                              = "session-stream-api"
    image                             = "000000000000.dkr.ecr.eu-west-1.amazonaws.com/aex/session-stream-api@sha256:0000000000000000000000000000000000000000000000000000000000000000"
    cpu                               = 1024
    memory                            = 2048
    desired_count                     = 2
    stop_timeout                      = 30
    container_port                    = 8080
    health_check_grace_period_seconds = 60
    log_group_name                    = "/aex/dev/session-stream-api"
    log_retention_days                = 30
    execution_role_arn                = "arn:aws:iam::000000000000:role/aex-dev-ecs-execution"
    # `/api/sessions` and `/api/sessions/*` are two condition values because an
    # ALB pattern ending in `/*` does not match the bare collection: `POST
    # /api/sessions` and `GET /api/sessions` carry no session id.
    #
    # All twenty-four NDJSON routes sit under `/api/streams/` — the
    # session-scoped ones and the workspace-scoped ones alike — so the whole
    # stream surface is one condition value in one rule. The 10 band was the
    # stream's and the 20s the session API's; keeping them apart costs nothing
    # and keeps the two halves legible in the listener.
    rules = [
      {
        priority      = 10
        path_patterns = ["/api/streams/*"]
      },
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

  # The executor is addressed only through the private Cloud Map namespace. It
  # has no public target group, and the empty client list is intentional until
  # brain-mux has an owning Terraform composition and a task security group to
  # name here.
  tool_executor = {
    name               = "tool-executor"
    image              = "000000000000.dkr.ecr.eu-west-1.amazonaws.com/aex/tool-executor@sha256:0000000000000000000000000000000000000000000000000000000000000000"
    cpu                = 1024
    memory             = 2048
    desired_count      = 1
    stop_timeout       = 30
    container_port     = 8080
    log_group_name     = "/aex/dev/tool-executor"
    log_retention_days = 30
    execution_role_arn = "arn:aws:iam::000000000000:role/aex-dev-ecs-execution"
    env = {
      AEX_TOOL_EXECUTOR_PLANE                = "dev"
      AEX_TOOL_EXECUTOR_REGION               = "eu-west-1"
      AEX_TOOL_EXECUTOR_PORT                 = "8080"
      AEX_TOOL_EXECUTOR_CEILING_TABLE        = "aex-dev-euw1-tool-executor-ceiling"
      AEX_TOOL_EXECUTOR_CREDENTIAL_SECRET_ID = "aex/dev/tool-executor/web-search"
      AEX_TOOL_EXECUTOR_MANIFEST             = "sha256:0000000000000000000000000000000000000000000000000000000000000000"
      AEX_TOOL_EXECUTOR_VERIFICATION_KEYS    = "00000000-0000-4000-8000-000000000000:0000000000000000000000000000000000000000000000000000000000000000:4102444800000"
    }
    client_security_group_ids = []
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

  alb = {
    name               = "aex-dev-euw1-public"
    certificate_arn    = "arn:aws:acm:eu-west-1:000000000000:certificate/00000000-0000-4000-8000-000000000000"
    access_logs_bucket = "aex-dev-alb-logs-0a1b2c3d"
  }

  deployable_grants = {
    "session-stream-api" = {
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

    "tool-executor" = {
      assume_principal = {
        type        = "Service"
        identifiers = ["ecs-tasks.amazonaws.com"]
      }
      wildcard_resource_allowlist = []
      action_grants = [
        {
          sid              = "OrganizationCeiling"
          actions          = ["dynamodb:TransactWriteItems"]
          resources        = ["arn:aws:dynamodb:eu-west-1:000000000000:table/aex-dev-euw1-tool-executor-ceiling"]
          scopable         = true
          condition_key    = "aws:ResourceTag/aex:plane"
          condition_values = ["dev"]
        },
        {
          sid              = "PlatformCredential"
          actions          = ["secretsmanager:GetSecretValue"]
          resources        = ["arn:aws:secretsmanager:eu-west-1:000000000000:secret:aex/dev/tool-executor/web-search-*"]
          scopable         = true
          condition_key    = "aws:ResourceTag/aex:plane"
          condition_values = ["dev"]
        },
      ]
    }
  }
}

run "the_region_application_plans" {
  command = plan

  assert {
    condition     = length(module.role) == 2
    error_message = "One execution role must be created for session-stream-api and one for tool-executor; the latter may not inherit the public request path's authority."
  }

  assert {
    condition     = jsondecode(module.role["session-stream-api"].inline_policy_json).Statement[1].Condition["ForAllValues:StringLike"]["dynamodb:LeadingKeys"] == ["SESSION#*"]
    error_message = "The region wrapper must preserve reviewed set-qualified conditions."
  }

  assert {
    condition = (
      contains(var.deployable_grants["tool-executor"].assume_principal.identifiers, "ecs-tasks.amazonaws.com")
      && !contains(var.deployable_grants["tool-executor"].assume_principal.identifiers, "lambda.amazonaws.com")
    )
    error_message = "The tool executor is a Fargate service and its role must be assumable only by ECS tasks."
  }
}

run "the_cluster_and_both_log_groups_are_created_by_this_root" {
  command = plan

  assert {
    condition     = module.cluster.name == var.cluster_name
    error_message = "The ECS cluster must be created under the configured name."
  }

  assert {
    condition     = module.session_stream_log_group.name == var.session_stream_api.log_group_name
    error_message = "The service log group must be created by this root, not assumed to exist. A Fargate service has no managed group waiting for it the way a Lambda did."
  }

  assert {
    condition     = module.tool_executor_log_group.name == var.tool_executor.log_group_name
    error_message = "The private executor must have its own log group; sharing the public request-path group would erase the deployable boundary in operations."
  }
}

run "the_tool_executor_is_private_named_and_fixed_count" {
  command = plan

  assert {
    condition     = module.internal_names.namespace_name == var.internal_namespace
    error_message = "The executor's Cloud Map namespace must be the private namespace selected by the regional composition."
  }

  assert {
    condition     = module.internal_names.hostnames["tool-executor"] == "tool-executor.${var.internal_namespace}"
    error_message = "The executor must have one stable private name; a Fargate task address changes on every deployment."
  }

  assert {
    condition = (
      can(regex("@sha256:[0-9a-f]{64}$", var.tool_executor.image))
      && var.tool_executor.cpu == 1024
      && var.tool_executor.memory == 2048
      && var.tool_executor.desired_count == 1
      && var.tool_executor.stop_timeout == 30
      && var.tool_executor.container_port == 8080
    )
    error_message = "The executor must carry the digest-pinned 1024/2048 single-task shape and 30-second deadline bound declared in release/units.toml."
  }

  assert {
    condition     = length(var.tool_executor.client_security_group_ids) == 0
    error_message = "Until brain-mux has an owning Terraform task group, the executor must admit no caller instead of widening ingress to a CIDR or the VPC."
  }

  assert {
    condition = toset(keys(var.tool_executor.env)) == toset([
      "AEX_TOOL_EXECUTOR_PLANE",
      "AEX_TOOL_EXECUTOR_REGION",
      "AEX_TOOL_EXECUTOR_PORT",
      "AEX_TOOL_EXECUTOR_CEILING_TABLE",
      "AEX_TOOL_EXECUTOR_CREDENTIAL_SECRET_ID",
      "AEX_TOOL_EXECUTOR_MANIFEST",
      "AEX_TOOL_EXECUTOR_VERIFICATION_KEYS",
    ])
    error_message = "The example must supply exactly the required tool-executor startup environment; the binary has no defaults and refuses partial configuration."
  }
}

run "the_service_drain_window_matches_its_target_group" {
  command = plan

  assert {
    condition     = module.session_stream_service.deregistration_delay == module.session_stream_target.deregistration_delay
    error_message = "The service and the target group in front of it must agree on the drain window."
  }

  assert {
    condition     = module.session_stream_service.deregistration_delay >= 30
    error_message = "The service drain window must be at least the target group deregistration delay."
  }
}

run "the_one_request_path_service_attaches_to_the_one_public_listener" {
  command = plan

  # These were cross-service checks while the unary API and the stream were two
  # deployables. They are within-service checks now, and they still matter: the
  # prefixes must stay disjoint so that splitting the halves back apart remains a
  # deployment change rather than a contract change.
  assert {
    condition     = length(flatten([for r in var.session_stream_api.rules : r.path_patterns])) == length(distinct(flatten([for r in var.session_stream_api.rules : r.path_patterns])))
    error_message = "No two rules may claim the same path pattern."
  }

  assert {
    condition = alltrue([
      for p in flatten([for r in var.session_stream_api.rules : r.path_patterns]) :
      startswith(p, "/api/") && !startswith(p, "/internal")
    ])
    error_message = "Every forwarded pattern must be a public /api/ path; /internal/* stays unreachable from the public listener, which is where both halves' health surfaces live."
  }

  assert {
    condition     = length(distinct(module.session_stream_target.rule_priorities)) == length(module.session_stream_target.rule_priorities)
    error_message = "Each listener rule must occupy its own priority."
  }

  # The stream half's whole surface is one prefix, so one rule carries it. The
  # per-rule quota still bounds a rule rather than a service, which the unary
  # half's two rules and the module's own tests exercise.
  assert {
    condition     = length(module.session_stream_target.rule_priorities) == 3
    error_message = "`/api/streams/*` is one condition value and therefore one rule; the unary half needs two more."
  }

  # The property the API surface supplies and this root depends on: the stream
  # half's first path segments and the unary half's are disjoint. A prefix match
  # is therefore sufficient to route, correctness does not rest on rule
  # precedence, and — the reason it still matters after the merge — splitting the
  # two halves back into two services stays a deployment change rather than a
  # contract change.
  #
  # This could not hold while `/api/sessions/{sessionId}/` was served by three
  # artifacts at once, nor while `/api/logs/query` and `/api/logs/stream` went to
  # different ones: in both cases only a segment after the shared prefix told
  # them apart.
  assert {
    condition = length(setintersection(
      toset([
        for p in flatten([for r in var.session_stream_api.rules : r.path_patterns]) :
        split("/", p)[2] if startswith(p, "/api/streams")
      ]),
      toset([
        for p in flatten([for r in var.session_stream_api.rules : r.path_patterns]) :
        split("/", p)[2] if !startswith(p, "/api/streams")
      ]),
    )) == 0
    error_message = "The NDJSON half and the unary half must not share a first path segment. If they do, no prefix separates them, routing falls back to rule precedence, and splitting them back apart would need a contract change rather than a second unit row."
  }

  # No pattern may wildcard the first segment, or the anchoring above is vacuous.
  assert {
    condition = alltrue([
      for p in flatten([for r in var.session_stream_api.rules : r.path_patterns]) :
      !strcontains(split("/", p)[2], "*") && !strcontains(split("/", p)[2], "?")
    ])
    error_message = "A forwarded pattern must name its first path segment literally; an ALB `*` matches across `/`, so a wildcard there would swallow another prefix."
  }
}

run "the_merged_service_runs_as_one_fargate_task_group_not_two" {
  command = plan

  assert {
    condition     = can(regex("@sha256:[0-9a-f]{64}$", var.session_stream_api.image))
    error_message = "The service image must be digest-pinned."
  }

  # The whole point of the merge: one task pair carries both halves, so the
  # regional floor is two tasks rather than the four two services cost.
  assert {
    condition = (
      var.session_stream_api.cpu == 1024
      && var.session_stream_api.memory == 2048
      && var.session_stream_api.desired_count == 2
      && var.session_stream_api.stop_timeout == 30
      && var.session_stream_api.container_port == 8080
    )
    error_message = "The service must carry the reviewed Fargate shape from release/units.toml: 1024/2048, two tasks, a 30 second stop timeout on port 8080."
  }

  assert {
    condition = (
      var.session_stream_api.autoscaling_bounds.min_capacity == var.session_stream_api.desired_count
      && var.session_stream_api.autoscaling_bounds.max_capacity == var.session_stream_api.desired_count
      && length(var.session_stream_api.autoscaling_metrics) == 0
    )
    error_message = "This example must use two fixed tasks without inventing a custom autoscaling metric."
  }

  assert {
    condition     = module.session_stream_service.deregistration_delay >= 30
    error_message = "The service must materialize as an ECS service behind a target group, with a drain window of at least 30 seconds."
  }

  assert {
    condition     = module.session_stream_log_group.name == "/aex/${var.plane}/${var.session_stream_api.name}"
    error_message = "The service must log to its own plane-qualified group."
  }

  assert {
    condition = (
      contains(var.deployable_grants["session-stream-api"].assume_principal.identifiers, "ecs-tasks.amazonaws.com")
      && !contains(var.deployable_grants["session-stream-api"].assume_principal.identifiers, "lambda.amazonaws.com")
    )
    error_message = "A Fargate task assumes its role as ecs-tasks.amazonaws.com. The Lambda trust principal it used to carry would leave the task unable to assume the role at all, and the service would never start."
  }
}
