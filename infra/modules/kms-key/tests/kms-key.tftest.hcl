mock_provider "aws" {}

variables {
  alias                     = "alias/aex-session-content"
  description               = "Session content envelope key."
  encryption_context_equals = { "aex:plane" = "dev" }

  policy_statements = [
    {
      sid            = "AccountAdministration"
      effect         = "Allow"
      principal_type = "AWS"
      principals     = ["arn:aws:iam::000000000000:root"]
      actions = [
        "kms:Create*", "kms:Describe*", "kms:Enable*", "kms:List*",
        "kms:Put*", "kms:Update*", "kms:Revoke*", "kms:Disable*",
        "kms:Get*", "kms:ScheduleKeyDeletion", "kms:CancelKeyDeletion",
      ]
      resources  = ["*"]
      data_plane = false
    },
    {
      sid                          = "SessionDataPlane"
      effect                       = "Allow"
      principal_type               = "AWS"
      principals                   = ["arn:aws:iam::000000000000:role/aex-regional-session-api"]
      actions                      = ["kms:Encrypt", "kms:Decrypt", "kms:GenerateDataKey"]
      resources                    = ["*"]
      data_plane                   = true
      encryption_context_workspace = "$${aws:PrincipalTag/aex:workspace}"
    },
    {
      sid            = "DelegateDataActionsToAccountIam"
      effect         = "Allow"
      principal_type = "AWS"
      principals     = ["arn:aws:iam::000000000000:root"]
      actions        = ["kms:Decrypt", "kms:GenerateDataKey", "kms:CreateGrant"]
      resources      = ["*"]
      data_plane     = false
      conditions = [
        {
          test     = "StringEquals"
          variable = "kms:ViaService"
          values   = ["dynamodb.eu-west-1.amazonaws.com", "s3.eu-west-1.amazonaws.com"]
        },
        {
          test     = "StringEquals"
          variable = "kms:ViaService"
          values   = ["sqs.eu-west-1.amazonaws.com"]
        },
        {
          test     = "StringEquals"
          variable = "kms:CallerAccount"
          values   = ["000000000000"]
        },
      ]
    },
    {
      sid            = "CloudWatchLogsEncryption"
      effect         = "Allow"
      principal_type = "Service"
      principals     = ["logs.eu-west-1.amazonaws.com"]
      actions        = ["kms:Encrypt", "kms:Decrypt", "kms:GenerateDataKey", "kms:DescribeKey"]
      resources      = ["*"]
      data_plane     = false
      conditions = [
        {
          test     = "ArnLike"
          variable = "kms:EncryptionContext:aws:logs:arn"
          values   = ["arn:aws:logs:eu-west-1:000000000000:log-group:/aex/dev/*"]
        },
      ]
    },
  ]
}

run "rotation_is_on_and_deletion_window_is_at_least_thirty_days" {
  command = plan

  assert {
    condition     = aws_kms_key.this.enable_key_rotation == true
    error_message = "Automatic key rotation must be enabled on the key resource."
  }

  assert {
    condition     = aws_kms_key.this.deletion_window_in_days >= 30
    error_message = "The pending-deletion window must be at least 30 days."
  }

  assert {
    condition     = aws_kms_alias.this.name == var.alias
    error_message = "The alias must be created with the requested name."
  }
}

run "no_kms_wildcard_action_on_wildcard_resource" {
  command = plan

  assert {
    condition = alltrue([
      for s in jsondecode(aws_kms_key.this.policy).Statement :
      !(contains(s.Action, "kms:*") && contains(s.Resource, "*"))
    ])
    error_message = "No statement may grant kms:* on Resource *."
  }

  assert {
    condition = alltrue([
      for s in jsondecode(aws_kms_key.this.policy).Statement :
      !contains(s.Action, "*")
    ])
    error_message = "No statement may grant Action *."
  }
}

run "every_data_plane_grant_carries_the_workspace_encryption_context" {
  command = plan

  assert {
    condition = length([
      for s in jsondecode(aws_kms_key.this.policy).Statement : s
      if can(s.Condition.StringEquals["kms:EncryptionContext:aex:workspace"])
      ]) == length([
      for s in var.policy_statements : s if s.data_plane
    ])
    error_message = "Exactly the data-plane grants must carry the workspace encryption-context condition."
  }

  assert {
    condition     = jsondecode(aws_kms_key.this.policy).Statement[1].Condition.StringEquals["kms:EncryptionContext:aex:plane"] == "dev"
    error_message = "Extra required encryption-context pairs must be rendered onto every data-plane grant."
  }
}

run "rejects_a_deletion_window_below_thirty_days" {
  command = plan

  variables {
    deletion_window_days = 7
  }

  expect_failures = [var.deletion_window_days]
}

run "rejects_disabling_rotation" {
  command = plan

  variables {
    rotation = false
  }

  expect_failures = [var.rotation]
}

run "rejects_a_data_plane_grant_without_an_encryption_context" {
  command = plan

  variables {
    policy_statements = [
      {
        sid            = "AccountAdministration"
        effect         = "Allow"
        principal_type = "AWS"
        principals     = ["arn:aws:iam::000000000000:root"]
        actions        = ["kms:Describe*"]
        resources      = ["*"]
        data_plane     = false
      },
      {
        sid            = "UnscopedDataPlane"
        effect         = "Allow"
        principal_type = "AWS"
        principals     = ["arn:aws:iam::000000000000:role/aex-regional-session-api"]
        actions        = ["kms:Decrypt"]
        resources      = ["*"]
        data_plane     = true
      },
    ]
  }

  expect_failures = [var.policy_statements]
}

run "rejects_kms_wildcard_on_wildcard_resource" {
  command = plan

  variables {
    policy_statements = [
      {
        sid            = "TooBroad"
        effect         = "Allow"
        principal_type = "AWS"
        principals     = ["arn:aws:iam::000000000000:root"]
        actions        = ["kms:*"]
        resources      = ["*"]
        data_plane     = false
      },
    ]
  }

  expect_failures = [var.policy_statements]
}

run "rejects_a_wildcard_principal" {
  command = plan

  variables {
    policy_statements = [
      {
        sid            = "AnyPrincipal"
        effect         = "Allow"
        principal_type = "AWS"
        principals     = ["*"]
        actions        = ["kms:Decrypt"]
        resources      = ["*"]
        data_plane     = false
      },
    ]
  }

  expect_failures = [var.policy_statements]
}

run "rejects_an_alias_outside_the_aex_namespace" {
  command = plan

  variables {
    alias = "alias/unrelated-key"
  }

  expect_failures = [var.alias]
}

run "rejects_a_workspace_pair_in_the_shared_encryption_context" {
  command = plan

  variables {
    encryption_context_equals = { "aex:workspace" = "ws-fixed" }
  }

  expect_failures = [var.encryption_context_equals]
}

run "a_statement_carries_every_condition_it_declares" {
  command = plan

  assert {
    condition = length([
      for s in jsondecode(aws_kms_key.this.policy).Statement : s
      if s.Sid == "DelegateDataActionsToAccountIam"
      && length(s.Condition.StringEquals["kms:ViaService"]) == 3
      && s.Condition.StringEquals["kms:CallerAccount"] == ["000000000000"]
    ]) == 1
    error_message = "A statement must be able to carry more than one condition, and repeated operator and variable pairs must merge their values rather than one replacing the other."
  }

  assert {
    condition = length([
      for s in jsondecode(aws_kms_key.this.policy).Statement : s
      if s.Sid == "CloudWatchLogsEncryption"
      && s.Condition.ArnLike["kms:EncryptionContext:aws:logs:arn"] == ["arn:aws:logs:eu-west-1:000000000000:log-group:/aex/dev/*"]
    ]) == 1
    error_message = "A service principal must be scopable by its own encryption-context key."
  }

  assert {
    condition = length([
      for s in jsondecode(aws_kms_key.this.policy).Statement : s
      if s.Sid == "AccountAdministration" && can(s.Condition)
    ]) == 0
    error_message = "A statement that declares no condition must render no Condition block at all."
  }
}

run "a_data_plane_grant_may_also_carry_its_own_conditions" {
  command = plan

  variables {
    policy_statements = [
      {
        sid            = "AccountAdministration"
        effect         = "Allow"
        principal_type = "AWS"
        principals     = ["arn:aws:iam::000000000000:root"]
        actions        = ["kms:Describe*"]
        resources      = ["*"]
        data_plane     = false
      },
      {
        sid                          = "ScopedDataPlane"
        effect                       = "Allow"
        principal_type               = "AWS"
        principals                   = ["arn:aws:iam::000000000000:role/aex-regional-secret-api"]
        actions                      = ["kms:Decrypt"]
        resources                    = ["*"]
        data_plane                   = true
        encryption_context_workspace = "$${aws:PrincipalTag/aex:workspace}"
        conditions = [
          {
            test     = "StringEquals"
            variable = "aws:RequestedRegion"
            values   = ["eu-west-1"]
          },
        ]
      },
    ]
  }

  assert {
    condition = length([
      for s in jsondecode(aws_kms_key.this.policy).Statement : s
      if s.Sid == "ScopedDataPlane"
      && can(s.Condition.StringEquals["kms:EncryptionContext:aex:workspace"])
      && s.Condition.StringEquals["aws:RequestedRegion"] == ["eu-west-1"]
    ]) == 1
    error_message = "The tenant encryption context must merge with the statement's own StringEquals conditions instead of replacing them."
  }
}

run "rejects_an_unconditioned_service_principal" {
  command = plan

  variables {
    policy_statements = [
      {
        sid            = "AccountAdministration"
        effect         = "Allow"
        principal_type = "AWS"
        principals     = ["arn:aws:iam::000000000000:root"]
        actions        = ["kms:Describe*"]
        resources      = ["*"]
        data_plane     = false
      },
      {
        sid            = "UnscopedService"
        effect         = "Allow"
        principal_type = "Service"
        principals     = ["logs.eu-west-1.amazonaws.com"]
        actions        = ["kms:Encrypt"]
        resources      = ["*"]
        data_plane     = false
      },
    ]
  }

  expect_failures = [var.policy_statements]
}

run "rejects_a_condition_operator_that_can_be_skipped" {
  command = plan

  variables {
    policy_statements = [
      {
        sid            = "SkippableCondition"
        effect         = "Allow"
        principal_type = "AWS"
        principals     = ["arn:aws:iam::000000000000:root"]
        actions        = ["kms:Decrypt"]
        resources      = ["*"]
        data_plane     = false
        conditions = [
          {
            test     = "StringEqualsIfExists"
            variable = "kms:ViaService"
            values   = ["s3.eu-west-1.amazonaws.com"]
          },
        ]
      },
    ]
  }

  expect_failures = [var.policy_statements]
}

run "rejects_a_wildcard_condition_value" {
  command = plan

  variables {
    policy_statements = [
      {
        sid            = "WildcardCondition"
        effect         = "Allow"
        principal_type = "AWS"
        principals     = ["arn:aws:iam::000000000000:root"]
        actions        = ["kms:Decrypt"]
        resources      = ["*"]
        data_plane     = false
        conditions = [
          {
            test     = "StringEquals"
            variable = "kms:ViaService"
            values   = ["*"]
          },
        ]
      },
    ]
  }

  expect_failures = [var.policy_statements]
}

run "rejects_two_statements_sharing_one_statement_id" {
  command = plan

  variables {
    policy_statements = [
      {
        sid            = "Twice"
        effect         = "Allow"
        principal_type = "AWS"
        principals     = ["arn:aws:iam::000000000000:root"]
        actions        = ["kms:Describe*"]
        resources      = ["*"]
        data_plane     = false
      },
      {
        sid            = "Twice"
        effect         = "Allow"
        principal_type = "AWS"
        principals     = ["arn:aws:iam::000000000000:root"]
        actions        = ["kms:Decrypt"]
        resources      = ["*"]
        data_plane     = false
        conditions = [
          {
            test     = "StringEquals"
            variable = "kms:ViaService"
            values   = ["s3.eu-west-1.amazonaws.com"]
          },
        ]
      },
    ]
  }

  expect_failures = [var.policy_statements]
}

run "rejects_the_tenant_context_supplied_as_a_free_form_condition" {
  command = plan

  variables {
    policy_statements = [
      {
        sid            = "TwoWaysToSayIt"
        effect         = "Allow"
        principal_type = "AWS"
        principals     = ["arn:aws:iam::000000000000:role/aex-regional-secret-api"]
        actions        = ["kms:Decrypt"]
        resources      = ["*"]
        data_plane     = false
        conditions = [
          {
            test     = "StringEquals"
            variable = "kms:EncryptionContext:aex:workspace"
            values   = ["wsp_0000000001e40r2081040g2081"]
          },
        ]
      },
    ]
  }

  expect_failures = [var.policy_statements]
}
