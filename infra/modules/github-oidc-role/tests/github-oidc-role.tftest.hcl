mock_provider "aws" {}

variables {
  role_name           = "aex-dev-release-publish"
  repository          = "example-owner/example-repo"
  oidc_provider_arn   = "arn:aws:iam::000000000000:oidc-provider/token.actions.githubusercontent.com"
  allowed_refs        = ["refs/heads/main"]
  allowed_workflows   = [".github/workflows/main.yml"]
  permissions_profile = "publish"

  profile_statements = {
    publish = [
      {
        sid       = "PublishObjects"
        actions   = ["s3:PutObject"]
        resources = ["arn:aws:s3:::aex-infra-artifacts-dev-0a1b2c3d/*"]
      },
      {
        sid       = "AuthenticateEcr"
        actions   = ["ecr:GetAuthorizationToken"]
        resources = ["*"]
      },
    ]
    plan = [
      {
        sid       = "ReadState"
        actions   = ["s3:GetObject"]
        resources = ["arn:aws:s3:::aex-dev-tfstate-0a1b2c3d/*"]
      },
    ]
    deploy = [
      {
        sid       = "DeployFunctions"
        actions   = ["lambda:UpdateFunctionCode"]
        resources = ["arn:aws:lambda:eu-west-1:000000000000:function:aex-dev-regional-session-api"]
      },
    ]
    readonly = [
      {
        sid       = "ReadLogs"
        actions   = ["logs:FilterLogEvents"]
        resources = ["arn:aws:logs:eu-west-1:000000000000:log-group:/aex/dev/*"]
      },
    ]
  }
}

run "trust_policy_names_the_exact_repository_ref_and_workflow" {
  command = plan

  assert {
    condition = alltrue([
      for subject in jsondecode(aws_iam_role.this.assume_role_policy).Statement[0].Condition.StringEquals["token.actions.githubusercontent.com:sub"] :
      subject == "repo:example-owner/example-repo:ref:refs/heads/main"
    ])
    error_message = "The sub condition must name the exact repository and ref."
  }

  assert {
    condition = alltrue([
      for workflow in jsondecode(aws_iam_role.this.assume_role_policy).Statement[0].Condition.StringEquals["token.actions.githubusercontent.com:job_workflow_ref"] :
      workflow == "example-owner/example-repo/.github/workflows/main.yml@refs/heads/main"
    ])
    error_message = "The job workflow reference condition must name the exact workflow path."
  }

  assert {
    condition     = jsondecode(aws_iam_role.this.assume_role_policy).Statement[0].Condition.StringEquals["token.actions.githubusercontent.com:aud"] == "sts.amazonaws.com"
    error_message = "The audience must be pinned to sts.amazonaws.com."
  }
}

run "keeps_unscopable_and_resource_scoped_actions_in_separate_statements" {
  command = plan

  assert {
    condition = jsondecode(aws_iam_role_policy.this.policy).Statement == [
      {
        Sid      = "PublishObjects"
        Effect   = "Allow"
        Action   = ["s3:PutObject"]
        Resource = ["arn:aws:s3:::aex-infra-artifacts-dev-0a1b2c3d/*"]
      },
      {
        Sid      = "AuthenticateEcr"
        Effect   = "Allow"
        Action   = ["ecr:GetAuthorizationToken"]
        Resource = ["*"]
      },
    ]
    error_message = "The policy must preserve each explicit action/resource pairing."
  }
}

run "rejects_overlapping_publish_and_deploy_profiles" {
  command = plan

  variables {
    profile_statements = {
      publish  = [{ sid = "Publish", actions = ["lambda:UpdateFunctionCode"], resources = ["*"] }]
      plan     = [{ sid = "Plan", actions = ["s3:GetObject"], resources = ["*"] }]
      deploy   = [{ sid = "Deploy", actions = ["lambda:UpdateFunctionCode"], resources = ["*"] }]
      readonly = [{ sid = "Read", actions = ["s3:GetObject"], resources = ["*"] }]
    }
  }

  expect_failures = [var.profile_statements]
}

run "rejects_a_wildcard_ref" {
  command = plan

  variables {
    allowed_refs = ["refs/heads/*"]
  }

  expect_failures = [var.allowed_refs]
}

run "rejects_a_short_ref" {
  command = plan

  variables {
    allowed_refs = ["main"]
  }

  expect_failures = [var.allowed_refs]
}

run "rejects_a_workflow_glob" {
  command = plan

  variables {
    allowed_workflows = [".github/workflows/*"]
  }

  expect_failures = [var.allowed_workflows]
}

run "rejects_a_wildcard_repository" {
  command = plan

  variables {
    repository = "example-owner/*"
  }

  expect_failures = [var.repository]
}

run "rejects_a_profile_with_a_wildcard_action" {
  command = plan

  variables {
    profile_statements = {
      publish  = [{ sid = "Publish", actions = ["s3:*"], resources = ["*"] }]
      plan     = [{ sid = "Plan", actions = ["s3:GetObject"], resources = ["*"] }]
      deploy   = [{ sid = "Deploy", actions = ["lambda:UpdateFunctionCode"], resources = ["*"] }]
      readonly = [{ sid = "Read", actions = ["s3:GetObject"], resources = ["*"] }]
    }
  }

  expect_failures = [var.profile_statements]
}

run "rejects_wildcard_mixed_with_exact_resources" {
  command = plan

  variables {
    profile_statements = {
      publish  = [{ sid = "Publish", actions = ["s3:PutObject"], resources = ["*", "arn:aws:s3:::bucket/*"] }]
      plan     = [{ sid = "Plan", actions = ["s3:GetObject"], resources = ["*"] }]
      deploy   = [{ sid = "Deploy", actions = ["lambda:UpdateFunctionCode"], resources = ["*"] }]
      readonly = [{ sid = "Read", actions = ["s3:GetObject"], resources = ["*"] }]
    }
  }

  expect_failures = [var.profile_statements]
}

run "an_environment_bound_role_excludes_the_unapproved_ref_subject" {
  command = plan

  variables {
    role_name            = "aex-prd-release-deploy"
    permissions_profile  = "deploy"
    allowed_environments = ["aex-prd"]
  }

  assert {
    condition = jsondecode(aws_iam_role.this.assume_role_policy).Statement[0].Condition.StringEquals["token.actions.githubusercontent.com:sub"] == [
      "repo:example-owner/example-repo:environment:aex-prd"
    ]
    error_message = "A protected deploy role must require the environment subject and exclude the unapproved ref subject."
  }
}

run "rejects_an_unowned_environment" {
  command = plan

  variables {
    allowed_environments = ["production"]
  }

  expect_failures = [var.allowed_environments]
}

run "rejects_a_role_name_without_a_plane_scope" {
  command = plan

  variables {
    role_name = "github-release"
  }

  expect_failures = [var.role_name]
}
