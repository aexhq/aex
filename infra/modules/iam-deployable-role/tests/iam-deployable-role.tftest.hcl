mock_provider "aws" {}

variables {
  deployable = "regional-session-api"
  plane      = "dev"
  region     = "eu-west-1"

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
    {
      sid       = "Entropy"
      actions   = ["kms:GenerateRandom"]
      resources = ["*"]
      scopable  = false
    },
  ]
}

run "trust_policy_names_an_explicit_principal" {
  command = plan

  assert {
    condition     = tolist(jsondecode(aws_iam_role.this.assume_role_policy).Statement[0].Principal.Service) == var.assume_principal.identifiers
    error_message = "The trust policy must name the supplied principals verbatim."
  }

  assert {
    condition     = jsondecode(aws_iam_role.this.assume_role_policy).Statement[0].Action == "sts:AssumeRole"
    error_message = "The trust policy must grant only sts:AssumeRole."
  }
}

run "inline_policy_has_no_wildcard_action" {
  command = plan

  assert {
    condition = alltrue([
      for s in jsondecode(aws_iam_role_policy.this.policy).Statement : !contains(s.Action, "*")
    ])
    error_message = "No statement may grant Action *."
  }

  assert {
    condition = alltrue(flatten([
      for s in jsondecode(aws_iam_role_policy.this.policy).Statement : [
        for a in s.Action : !endswith(a, ":*")
      ]
    ]))
    error_message = "No statement may grant a whole-service wildcard."
  }
}

run "wildcard_resource_is_confined_to_the_allowlist" {
  command = plan

  assert {
    condition = alltrue(flatten([
      for s in jsondecode(aws_iam_role_policy.this.policy).Statement : [
        for a in s.Action : contains(var.wildcard_resource_allowlist, a)
      ] if contains(s.Resource, "*")
    ]))
    error_message = "Resource * is only permitted for allowlisted actions."
  }
}

run "scopable_grants_carry_a_plane_or_region_condition" {
  command = plan

  assert {
    condition = alltrue([
      for i, g in var.action_grants :
      !g.scopable || can(jsondecode(aws_iam_role_policy.this.policy).Statement[i].Condition.StringEquals)
    ])
    error_message = "Every scopable grant must render a plane or region condition."
  }

  assert {
    condition     = jsondecode(aws_iam_role_policy.this.policy).Statement[0].Condition.StringEquals["aws:ResourceTag/aex:plane"] == ["dev"]
    error_message = "The plane condition must carry the configured plane value."
  }
}

run "rejects_a_wildcard_action" {
  command = plan

  variables {
    action_grants = [
      {
        sid       = "Everything"
        actions   = ["*"]
        resources = ["arn:aws:dynamodb:eu-west-1:000000000000:table/aex-dev-euw1-session-journal"]
        scopable  = false
      },
    ]
  }

  expect_failures = [var.action_grants]
}

run "rejects_a_whole_service_wildcard" {
  command = plan

  variables {
    action_grants = [
      {
        sid       = "AllDynamo"
        actions   = ["dynamodb:*"]
        resources = ["arn:aws:dynamodb:eu-west-1:000000000000:table/aex-dev-euw1-session-journal"]
        scopable  = false
      },
    ]
  }

  expect_failures = [var.action_grants]
}

run "rejects_a_wildcard_resource_for_an_action_outside_the_allowlist" {
  command = plan

  variables {
    action_grants = [
      {
        sid       = "ReadAnything"
        actions   = ["s3:GetObject"]
        resources = ["*"]
        scopable  = false
      },
    ]
  }

  expect_failures = [var.action_grants]
}

run "rejects_a_scopable_grant_with_no_condition" {
  command = plan

  variables {
    action_grants = [
      {
        sid       = "UnscopedJournal"
        actions   = ["dynamodb:GetItem"]
        resources = ["arn:aws:dynamodb:eu-west-1:000000000000:table/aex-dev-euw1-session-journal"]
        scopable  = true
      },
    ]
  }

  expect_failures = [var.action_grants]
}

run "rejects_a_wildcard_trust_principal" {
  command = plan

  variables {
    assume_principal = {
      type        = "AWS"
      identifiers = ["*"]
    }
  }

  expect_failures = [var.assume_principal]
}

run "rejects_an_allowlist_that_is_itself_a_wildcard" {
  command = plan

  variables {
    wildcard_resource_allowlist = ["*"]
  }

  expect_failures = [var.wildcard_resource_allowlist]
}
