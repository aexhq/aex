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
    condition     = length(module.role) == 2
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
