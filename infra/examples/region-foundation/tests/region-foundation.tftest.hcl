mock_provider "aws" {}

variables {
  plane                       = "dev"
  region                      = "eu-west-1"
  name_prefix                 = "aex-dev-euw1-"
  keystore_physical_name      = "aex-keystore-v1"
  content_bucket_suffix       = "0a1b2c3d"
  content_lifecycle_role_arn  = "arn:aws:iam::000000000000:role/aex-dev-content-lifecycle"
  table_definitions_digest    = "sha256:1111111111111111111111111111111111111111111111111111111111111111"
  expected_definitions_digest = "sha256:1111111111111111111111111111111111111111111111111111111111111111"

  vpc = {
    name               = "aex-dev-euw1"
    cidr               = "10.40.0.0/16"
    az_count           = 2
    availability_zones = ["eu-west-1a", "eu-west-1b"]
    endpoints          = ["ecr.api", "ecr.dkr", "kms", "logs", "secretsmanager", "sts", "sqs"]
  }

  authority_keys = {
    session = {
      alias                     = "alias/aex-session-dev-euw1"
      description               = "Session authority key."
      encryption_context_equals = { "aex:plane" = "dev" }
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
        {
          sid                          = "SessionDataPlane"
          effect                       = "Allow"
          principal_type               = "AWS"
          principals                   = ["arn:aws:iam::000000000000:role/aex-dev-regional-session-api"]
          actions                      = ["kms:Encrypt", "kms:Decrypt", "kms:GenerateDataKey"]
          resources                    = ["*"]
          data_plane                   = true
          encryption_context_workspace = "$${aws:PrincipalTag/aex:workspace}"
        },
      ]
    }

    secret = {
      alias                     = "alias/aex-secret-dev-euw1"
      description               = "Secret authority key."
      encryption_context_equals = { "aex:plane" = "dev" }
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
      alias                     = "alias/aex-content-dev-euw1"
      description               = "Content authority key."
      encryption_context_equals = { "aex:plane" = "dev" }
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
      stream_view_type            = "NEW_AND_OLD_IMAGES"
      attributes = [
        { name = "pk", type = "S" },
        { name = "sk", type = "S" },
      ]
    },
    {
      logical_name                = "keystore"
      authority                   = "secret"
      hash_key                    = "pk"
      billing_mode                = "PAY_PER_REQUEST"
      point_in_time_recovery_days = 35
      deletion_protection         = true
      attributes                  = [{ name = "pk", type = "S" }]
    },
  ]
}

run "the_region_foundation_plans" {
  command = plan

  assert {
    condition     = module.content.bucket == "aex-content-dev-eu-west-1-0a1b2c3d"
    error_message = "The content bucket name must be derived from the plane, region and suffix."
  }

  assert {
    condition     = module.tables.table_names["keystore"] == var.keystore_physical_name
    error_message = "The keystore table must keep its pinned physical name."
  }
}

run "no_nat_gateway_in_the_regional_network" {
  command = plan

  assert {
    condition     = length(module.network.nat_gateway_ids) == 0
    error_message = "The regional network must have no NAT gateway."
  }
}

run "every_table_authority_has_a_key" {
  command = plan

  assert {
    condition = alltrue([
      for t in var.table_definitions : contains(keys(var.authority_keys), t.authority)
    ])
    error_message = "Every authority a table names must have a customer-managed key in this root."
  }
}
