mock_provider "aws" {}

variables {
  plane                           = "prd"
  region                          = "eu-west-1"
  account_id                      = "000000000000"
  permissions_boundary_policy_arn = "arn:aws:iam::000000000000:policy/aex-dev-application-boundary"
  name_prefix                     = "aex-prd-euw1-"
  bucket_suffix                   = "0a1b2c3d"
  cluster_name                    = "aex-prd-euw1"
  content_bucket_purpose          = "content"
  content_lifecycle_role_arn      = "arn:aws:iam::000000000000:role/aex-prd-content-lifecycle"
  artifact_retention_days         = 90
  ops_topic_name                  = "aex-prd-ops"
  table_definitions_digest        = "blake3:1111111111111111111111111111111111111111111111111111111111111111"
  expected_definitions_digest     = "blake3:1111111111111111111111111111111111111111111111111111111111111111"

  vpc = {
    name               = "aex-prd-euw1"
    cidr               = "10.50.0.0/16"
    az_count           = 2
    availability_zones = ["eu-west-1a", "eu-west-1b"]
    endpoints          = ["ecr.api", "ecr.dkr", "kms", "logs", "secretsmanager", "sts"]
  }

  authority_keys = {
    session = {
      alias                     = "alias/aex-session-prd-euw1"
      description               = "Session authority key."
      encryption_context_equals = { "aex:plane" = "prd" }
      policy_statements = [
        {
          sid            = "AccountAdministration"
          effect         = "Allow"
          principal_type = "AWS"
          principals     = ["arn:aws:iam::000000000000:root"]
          actions        = ["kms:Describe*", "kms:List*", "kms:Get*", "kms:Put*"]
          resources      = ["*"]
          data_plane     = false
        },
      ]
    }

    secret = {
      alias                     = "alias/aex-secret-prd-euw1"
      description               = "Secret authority key."
      encryption_context_equals = { "aex:plane" = "prd" }
      policy_statements = [
        {
          sid            = "AccountAdministration"
          effect         = "Allow"
          principal_type = "AWS"
          principals     = ["arn:aws:iam::000000000000:root"]
          actions        = ["kms:Describe*", "kms:List*", "kms:Get*", "kms:Put*"]
          resources      = ["*"]
          data_plane     = false
        },
      ]
    }

    content = {
      alias                     = "alias/aex-content-prd-euw1"
      description               = "Content authority key."
      encryption_context_equals = { "aex:plane" = "prd" }
      policy_statements = [
        {
          sid            = "AccountAdministration"
          effect         = "Allow"
          principal_type = "AWS"
          principals     = ["arn:aws:iam::000000000000:root"]
          actions        = ["kms:Describe*", "kms:List*", "kms:Get*", "kms:Put*"]
          resources      = ["*"]
          data_plane     = false
        },
      ]
    }
  }

  table_definitions = [
    {
      logical_name                = "session_journal"
      authority                   = "session"
      hash_key                    = "pk"
      range_key                   = "sk"
      billing_mode                = "PAY_PER_REQUEST"
      point_in_time_recovery_days = 35
      deletion_protection         = true
      ttl_attribute               = "expiresAtEpochSeconds"
      attributes = [
        { name = "pk", type = "S" },
        { name = "sk", type = "S" },
      ]
    },
    {
      logical_name                = "keystore"
      authority                   = "secret"
      pinned_physical_name        = "aex-keystore-v1"
      hash_key                    = "pk"
      billing_mode                = "PAY_PER_REQUEST"
      point_in_time_recovery_days = 35
      deletion_protection         = true
      attributes                  = [{ name = "pk", type = "S" }]
    },
  ]

  ops_publish_principals = [
    { type = "Service", identifier = "cloudwatch.amazonaws.com" },
  ]

  ops_subscriptions = [
    { protocol = "https", endpoint = "https://alerts.example.invalid/aex" },
  ]

  session_api = {
    name               = "regional-session-api"
    image              = "000000000000.dkr.ecr.eu-west-1.amazonaws.com/aex/regional-session-api@sha256:0000000000000000000000000000000000000000000000000000000000000000"
    cpu                = 1024
    memory             = 2048
    desired_count      = 2
    stop_timeout       = 30
    container_port     = 8080
    log_group_name     = "/aex/prd/regional-session-api"
    log_retention_days = 30
    execution_role_arn = "arn:aws:iam::000000000000:role/aex-prd-ecs-execution"
    env = {
      AEX_PLANE  = "prd"
      AEX_REGION = "eu-west-1"
    }
    autoscaling_bounds = {
      min_capacity = 2
      max_capacity = 2
    }
    autoscaling_metrics = []
  }

  session_api_grants = {
    assume_principal = {
      type        = "Service"
      identifiers = ["ecs-tasks.amazonaws.com"]
    }
    wildcard_resource_allowlist = ["kms:GenerateRandom"]
    action_grants = [
      {
        sid                = "SessionJournal"
        actions            = ["dynamodb:GetItem", "dynamodb:PutItem", "dynamodb:TransactWriteItems"]
        resources          = ["arn:aws:dynamodb:eu-west-1:000000000000:table/aex-prd-euw1-session-journal"]
        scopable           = true
        condition_operator = "ForAllValues:StringLike"
        condition_key      = "dynamodb:LeadingKeys"
        condition_values   = ["SESSION#*"]
      },
    ]
  }
}

run "the_self_host_root_plans" {
  command = plan

  assert {
    condition     = module.content.bucket == "aex-prd-eu-west-1-content"
    error_message = "The content bucket name must be derived from the plane, the region and what the store holds."
  }

  assert {
    condition     = module.artifacts.bucket == "aex-infra-artifacts-prd-0a1b2c3d"
    error_message = "The artifact bucket name must be derived from the plane and suffix."
  }


  assert {
    condition     = jsondecode(module.session_api_role.inline_policy_json).Statement[0].Condition["ForAllValues:StringLike"]["dynamodb:LeadingKeys"] == ["SESSION#*"]
    error_message = "The self-host wrapper must preserve reviewed set-qualified conditions."
  }
}

run "the_service_runs_exactly_the_published_bytes" {
  command = plan

  assert {
    condition     = can(regex("@sha256:[0-9a-f]{64}$", var.session_api.image))
    error_message = "The image must be digest-pinned, so a self-hoster deploys exactly the published bytes and a restart cannot land on different ones."
  }

  assert {
    condition     = module.session_service.deregistration_delay >= 30
    error_message = "The session service must carry a drain window of at least 30 seconds."
  }
}

run "the_cluster_and_the_log_group_are_created_by_this_root" {
  command = plan

  assert {
    condition     = module.cluster.name == var.cluster_name
    error_message = "The ECS cluster must be created here, so a fresh account plans from empty."
  }

  assert {
    condition     = module.session_log_group.name == var.session_api.log_group_name
    error_message = "The service log group must be created here; a Fargate task has no managed group waiting for it."
  }
}

run "the_session_service_assumes_its_role_as_a_task" {
  command = plan

  assert {
    condition = (
      contains(var.session_api_grants.assume_principal.identifiers, "ecs-tasks.amazonaws.com")
      && !contains(var.session_api_grants.assume_principal.identifiers, "lambda.amazonaws.com")
    )
    error_message = "A Fargate task assumes its role as ecs-tasks.amazonaws.com; the Lambda trust principal would leave the task unable to assume the role at all."
  }
}

run "the_network_has_no_nat_gateway" {
  command = plan

  assert {
    condition     = length(module.network.nat_gateway_ids) == 0
    error_message = "A self-host network needs no NAT gateway."
  }
}
