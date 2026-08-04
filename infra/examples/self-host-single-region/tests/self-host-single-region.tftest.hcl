mock_provider "aws" {
  mock_data "aws_s3_object" {
    defaults = {
      checksum_sha256 = "47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU="
    }
  }
}

variables {
  plane                           = "prd"
  region                          = "eu-west-1"
  account_id                      = "000000000000"
  permissions_boundary_policy_arn = "arn:aws:iam::000000000000:policy/aex-dev-application-boundary"
  name_prefix                     = "aex-prd-euw1-"
  bucket_suffix                   = "0a1b2c3d"
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
    function_name           = "aex-prd-regional-session-api"
    artifact_key            = "lambda/regional-session-api/e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855.zip"
    artifact_object_version = "aBcDeFgHiJkLmNoPqRsTuVwXyZ012345"
    artifact_sha256         = "47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU="
    memory_mb               = 512
    timeout_s               = 30
    log_retention_days      = 30
    env = {
      AEX_PLANE  = "prd"
      AEX_REGION = "eu-west-1"
    }
  }

  session_api_grants = {
    assume_principal = {
      type        = "Service"
      identifiers = ["lambda.amazonaws.com"]
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

run "the_lambda_reads_its_bytes_from_the_artifact_bucket_this_root_created" {
  command = plan

  assert {
    condition     = can(regex("^lambda/[a-z0-9-]+/[0-9a-f]{64}\\.zip$", var.session_api.artifact_key))
    error_message = "The artifact key must be digest-addressed, so a self-hoster deploys exactly the published bytes."
  }
}

run "the_network_has_no_nat_gateway" {
  command = plan

  assert {
    condition     = length(module.network.nat_gateway_ids) == 0
    error_message = "A self-host network needs no NAT gateway."
  }
}
