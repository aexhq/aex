mock_provider "aws" {}

variables {
  plane                       = "dev"
  region                      = "eu-west-1"
  name_prefix                 = "aex-dev-euw1-"
  table_definitions_digest    = "blake3:1111111111111111111111111111111111111111111111111111111111111111"
  expected_definitions_digest = "blake3:1111111111111111111111111111111111111111111111111111111111111111"

  kms_key_arn_by_authority = {
    session = "arn:aws:kms:eu-west-1:000000000000:key/00000000-0000-4000-8000-000000000000"
    secret  = "arn:aws:kms:eu-west-1:000000000000:key/00000000-0000-4000-8000-000000000001"
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
        { name = "workspaceId", type = "S" },
      ]
      global_secondary_indexes = [
        {
          name               = "gsi_workspace_index"
          hash_key           = "workspaceId"
          range_key          = "sk"
          projection_type    = "INCLUDE"
          non_key_attributes = ["status"]
        },
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
}

run "every_table_is_on_demand_protected_and_encrypted_with_a_customer_key" {
  command = plan

  assert {
    condition = alltrue([
      for k, t in aws_dynamodb_table.this : t.billing_mode == "PAY_PER_REQUEST"
    ])
    error_message = "Every table must be PAY_PER_REQUEST."
  }

  assert {
    condition = alltrue([
      for k, t in aws_dynamodb_table.this : t.deletion_protection_enabled
    ])
    error_message = "Every table must have deletion protection enabled."
  }

  assert {
    condition = alltrue([
      for k, t in aws_dynamodb_table.this :
      t.point_in_time_recovery[0].enabled && t.point_in_time_recovery[0].recovery_period_in_days >= 35
    ])
    error_message = "Every table must keep 35 days of point-in-time recovery."
  }

  assert {
    condition = alltrue([
      for k, t in aws_dynamodb_table.this :
      t.server_side_encryption[0].enabled && startswith(t.server_side_encryption[0].kms_key_arn, "arn:aws:kms:")
    ])
    error_message = "Every table must be encrypted with a named customer-managed key, never the AWS-managed key."
  }
}

run "no_gsi_projects_all_attributes" {
  command = plan

  assert {
    condition = alltrue(flatten([
      for k, t in aws_dynamodb_table.this : [
        for g in t.global_secondary_index : g.projection_type != "ALL"
      ]
    ]))
    error_message = "No global secondary index may project ALL."
  }
}

run "ttl_attribute_is_the_canonical_name" {
  command = plan

  assert {
    condition = alltrue(flatten([
      for k, t in aws_dynamodb_table.this : [
        for x in t.ttl : x.attribute_name == "expiresAtEpochSeconds" if x.enabled
      ]
    ]))
    error_message = "The only permitted TTL attribute name is expiresAtEpochSeconds."
  }
}

run "a_pinned_definition_keeps_its_own_physical_name" {
  command = plan

  assert {
    condition     = aws_dynamodb_table.this["keystore"].name == "aex-keystore-v1"
    error_message = "A table declaring a pinned physical name must be created under exactly that name."
  }

  assert {
    condition     = aws_dynamodb_table.this["session_journal"].name == "aex-dev-euw1-session-journal"
    error_message = "Every table without a pinned name must be created under the environment name prefix."
  }

  assert {
    condition     = output.table_names["keystore"] == "aex-keystore-v1"
    error_message = "The logical-to-physical map must report the pinned name."
  }
}

run "a_pinned_name_is_keyed_on_the_definition_not_on_a_logical_name" {
  command = plan

  variables {
    table_definitions = [
      {
        logical_name                = "regional_secret_keystore"
        authority                   = "secret"
        pinned_physical_name        = "aex-regional-branch-keystore"
        hash_key                    = "pk"
        billing_mode                = "PAY_PER_REQUEST"
        point_in_time_recovery_days = 35
        deletion_protection         = true
        attributes                  = [{ name = "pk", type = "S" }]
      },
    ]
  }

  assert {
    condition     = aws_dynamodb_table.this["regional_secret_keystore"].name == "aex-regional-branch-keystore"
    error_message = "The pinned name must be honoured whatever the table is called; a special case on the literal logical name `keystore` never fires for a bundle that names the table something else."
  }
}

run "a_keys_only_index_projects_no_attribute" {
  command = plan

  variables {
    table_definitions = [
      {
        logical_name                = "observation_authority"
        authority                   = "session"
        hash_key                    = "pk"
        range_key                   = "sk"
        billing_mode                = "PAY_PER_REQUEST"
        point_in_time_recovery_days = 35
        deletion_protection         = true
        attributes = [
          { name = "pk", type = "S" },
          { name = "sk", type = "S" },
          { name = "cPk", type = "S" },
          { name = "cSk", type = "S" },
        ]
        global_secondary_indexes = [
          {
            name               = "gsi_control"
            hash_key           = "cPk"
            range_key          = "cSk"
            projection_type    = "KEYS_ONLY"
            non_key_attributes = []
          },
        ]
      },
    ]
  }

  assert {
    condition     = one(aws_dynamodb_table.this["observation_authority"].global_secondary_index).projection_type == "KEYS_ONLY"
    error_message = "A KEYS_ONLY index must reach the resource as KEYS_ONLY."
  }

  assert {
    condition     = length(coalesce(one(aws_dynamodb_table.this["observation_authority"].global_secondary_index).non_key_attributes, [])) == 0
    error_message = "A KEYS_ONLY index must carry no projected attribute list; AWS rejects the create when one is present."
  }
}

run "rejects_provisioned_capacity" {
  command = plan

  variables {
    table_definitions = [
      {
        logical_name                = "session_journal"
        authority                   = "session"
        hash_key                    = "pk"
        billing_mode                = "PROVISIONED"
        point_in_time_recovery_days = 35
        deletion_protection         = true
        attributes                  = [{ name = "pk", type = "S" }]
      },
    ]
  }

  expect_failures = [var.table_definitions]
}

run "rejects_point_in_time_recovery_below_thirty_five_days" {
  command = plan

  variables {
    table_definitions = [
      {
        logical_name                = "session_journal"
        authority                   = "session"
        hash_key                    = "pk"
        billing_mode                = "PAY_PER_REQUEST"
        point_in_time_recovery_days = 7
        deletion_protection         = true
        attributes                  = [{ name = "pk", type = "S" }]
      },
    ]
  }

  expect_failures = [var.table_definitions]
}

run "rejects_disabled_deletion_protection" {
  command = plan

  variables {
    table_definitions = [
      {
        logical_name                = "session_journal"
        authority                   = "session"
        hash_key                    = "pk"
        billing_mode                = "PAY_PER_REQUEST"
        point_in_time_recovery_days = 35
        deletion_protection         = false
        attributes                  = [{ name = "pk", type = "S" }]
      },
    ]
  }

  expect_failures = [var.table_definitions]
}

run "rejects_a_gsi_that_projects_all" {
  command = plan

  variables {
    table_definitions = [
      {
        logical_name                = "session_journal"
        authority                   = "session"
        hash_key                    = "pk"
        billing_mode                = "PAY_PER_REQUEST"
        point_in_time_recovery_days = 35
        deletion_protection         = true
        attributes = [
          { name = "pk", type = "S" },
          { name = "workspaceId", type = "S" },
        ]
        global_secondary_indexes = [
          {
            name            = "gsi_workspace_index"
            hash_key        = "workspaceId"
            projection_type = "ALL"
          },
        ]
      },
    ]
  }

  expect_failures = [var.table_definitions]
}

run "rejects_a_non_canonical_ttl_attribute" {
  command = plan

  variables {
    table_definitions = [
      {
        logical_name                = "session_journal"
        authority                   = "session"
        hash_key                    = "pk"
        billing_mode                = "PAY_PER_REQUEST"
        point_in_time_recovery_days = 35
        deletion_protection         = true
        ttl_attribute               = "ttl"
        attributes                  = [{ name = "pk", type = "S" }]
      },
    ]
  }

  expect_failures = [var.table_definitions]
}

run "rejects_an_authority_with_no_customer_managed_key" {
  command = plan

  variables {
    kms_key_arn_by_authority = {
      session = "arn:aws:kms:eu-west-1:000000000000:key/00000000-0000-4000-8000-000000000000"
    }
  }

  expect_failures = [var.kms_key_arn_by_authority]
}

run "rejects_a_pinned_name_derived_from_the_environment_prefix" {
  command = plan

  variables {
    table_definitions = [
      {
        logical_name                = "keystore"
        authority                   = "secret"
        pinned_physical_name        = "aex-dev-euw1-keystore"
        hash_key                    = "pk"
        billing_mode                = "PAY_PER_REQUEST"
        point_in_time_recovery_days = 35
        deletion_protection         = true
        attributes                  = [{ name = "pk", type = "S" }]
      },
    ]
  }

  expect_failures = [var.table_definitions]
}

run "rejects_an_include_projection_that_names_no_attribute" {
  command = plan

  variables {
    table_definitions = [
      {
        logical_name                = "session_journal"
        authority                   = "session"
        hash_key                    = "pk"
        billing_mode                = "PAY_PER_REQUEST"
        point_in_time_recovery_days = 35
        deletion_protection         = true
        attributes = [
          { name = "pk", type = "S" },
          { name = "workspaceId", type = "S" },
        ]
        global_secondary_indexes = [
          {
            name               = "gsi_workspace_index"
            hash_key           = "workspaceId"
            projection_type    = "INCLUDE"
            non_key_attributes = []
          },
        ]
      },
    ]
  }

  expect_failures = [var.table_definitions]
}

run "rejects_a_keys_only_projection_that_names_attributes" {
  command = plan

  variables {
    table_definitions = [
      {
        logical_name                = "session_journal"
        authority                   = "session"
        hash_key                    = "pk"
        billing_mode                = "PAY_PER_REQUEST"
        point_in_time_recovery_days = 35
        deletion_protection         = true
        attributes = [
          { name = "pk", type = "S" },
          { name = "workspaceId", type = "S" },
        ]
        global_secondary_indexes = [
          {
            name               = "gsi_workspace_index"
            hash_key           = "workspaceId"
            projection_type    = "KEYS_ONLY"
            non_key_attributes = ["status"]
          },
        ]
      },
    ]
  }

  expect_failures = [var.table_definitions]
}

run "rejects_a_definition_set_that_does_not_match_the_pinned_bundle" {
  command = plan

  variables {
    table_definitions_digest = "blake3:2222222222222222222222222222222222222222222222222222222222222222"
  }

  expect_failures = [var.table_definitions_digest]
}

run "rejects_a_digest_in_another_domain" {
  command = plan

  variables {
    table_definitions_digest = "sha256:1111111111111111111111111111111111111111111111111111111111111111"
  }

  expect_failures = [var.table_definitions_digest]
}
