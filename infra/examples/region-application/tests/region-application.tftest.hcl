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
  vpc_id                          = "vpc-0123456789abcdef0"
  private_subnet_ids              = ["subnet-0123456789abcdef0", "subnet-0123456789abcdef1"]
  public_subnet_ids               = ["subnet-0123456789abcdef2", "subnet-0123456789abcdef3"]
  service_security_group_ids      = ["sg-0123456789abcdef0"]
  alb_security_group_ids          = ["sg-0123456789abcdef1"]
  kms_key_arn                     = "arn:aws:kms:eu-west-1:000000000000:key/00000000-0000-4000-8000-000000000000"
  session_journal_stream_arn      = "arn:aws:dynamodb:eu-west-1:000000000000:table/aex-dev-euw1-session-journal/stream/2026-08-01T00:00:00.000"
  artifact_bucket                 = "aex-infra-artifacts-dev-0a1b2c3d"
  cluster_name                    = "aex-dev-euw1"

  session_api = {
    function_name           = "aex-dev-regional-session-api"
    artifact_key            = "lambda/regional-session-api/e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855.zip"
    artifact_object_version = "aBcDeFgHiJkLmNoPqRsTuVwXyZ012345"
    artifact_sha256         = "47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU="
    memory_mb               = 512
    timeout_s               = 30
    log_retention_days      = 30
    env = {
      AEX_PLANE  = "dev"
      AEX_REGION = "eu-west-1"
    }
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
    name               = "regional-stream"
    image              = "000000000000.dkr.ecr.eu-west-1.amazonaws.com/aex/regional-stream@sha256:0000000000000000000000000000000000000000000000000000000000000000"
    cpu                = 1024
    memory             = 2048
    stop_timeout       = 30
    container_port     = 8080
    log_group_name     = "/aex/dev/regional-stream"
    log_retention_days = 30
    execution_role_arn = "arn:aws:iam::000000000000:role/aex-dev-ecs-execution"
    env = {
      AEX_PLANE = "dev"
    }
    autoscaling_bounds = {
      min_capacity = 1
      max_capacity = 6
    }
    autoscaling_metrics = [
      {
        name         = "StreamBacklogSeconds"
        namespace    = "AEX/RegionalStream"
        statistic    = "Average"
        target_value = 5
      },
    ]
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
        identifiers = ["lambda.amazonaws.com"]
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

run "the_cluster_and_the_log_group_are_created_by_this_root" {
  command = plan

  assert {
    condition     = module.cluster.name == var.cluster_name
    error_message = "The ECS cluster must be created under the configured name."
  }

  assert {
    condition     = module.stream_log_group.name == var.stream_service.log_group_name
    error_message = "The service log group must be created by this root, not assumed to exist."
  }
}

run "the_service_drain_window_matches_the_target_group" {
  command = plan

  assert {
    condition     = module.stream_service.deregistration_delay >= 30
    error_message = "The service drain window must be at least the target group deregistration delay."
  }
}

run "the_stream_service_runs_a_digest_pinned_image" {
  command = plan

  assert {
    condition     = can(regex("@sha256:[0-9a-f]{64}$", var.stream_service.image))
    error_message = "The stream service image must be digest-pinned."
  }
}
