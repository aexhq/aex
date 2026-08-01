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
