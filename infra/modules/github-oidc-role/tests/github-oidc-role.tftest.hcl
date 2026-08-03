mock_provider "aws" {}

variables {
  role_name           = "aex-dev-release-publish"
  repository          = "example-owner/example-repo"
  oidc_provider_arn   = "arn:aws:iam::000000000000:oidc-provider/token.actions.githubusercontent.com"
  allowed_refs        = ["refs/heads/main"]
  allowed_workflows   = [".github/workflows/main.yml"]
  permissions_profile = "publish"

  permission_profiles = {
    publish  = ["s3:PutObject", "ecr:PutImage", "ecr:UploadLayerPart", "ecr:InitiateLayerUpload", "ecr:CompleteLayerUpload"]
    plan     = ["s3:GetObject", "dynamodb:DescribeTable"]
    deploy   = ["lambda:UpdateFunctionCode", "ecs:UpdateService", "lambda:PublishVersion"]
    readonly = ["s3:GetObject"]
  }

  profile_resources = {
    publish  = ["arn:aws:s3:::aex-infra-artifacts-dev-0a1b2c3d/*"]
    plan     = ["arn:aws:s3:::aex-tfstate-dev-0a1b2c3d/*"]
    deploy   = ["arn:aws:lambda:eu-west-1:000000000000:function:aex-dev-regional-session-api"]
    readonly = ["arn:aws:s3:::aex-infra-artifacts-dev-0a1b2c3d/*"]
  }
}

run "trust_policy_names_the_exact_repository_ref_and_workflow" {
  command = plan

  assert {
    condition = alltrue([
      for s in jsondecode(aws_iam_role.this.assume_role_policy).Statement[0].Condition.StringEquals["token.actions.githubusercontent.com:sub"] :
      s == "repo:example-owner/example-repo:ref:refs/heads/main"
    ])
    error_message = "The sub condition must name the exact repository and ref."
  }

  assert {
    condition = alltrue([
      for w in jsondecode(aws_iam_role.this.assume_role_policy).Statement[0].Condition.StringEquals["token.actions.githubusercontent.com:job_workflow_ref"] :
      w == "example-owner/example-repo/.github/workflows/main.yml@refs/heads/main"
    ])
    error_message = "The job workflow reference condition must name the exact workflow path."
  }

  assert {
    condition = alltrue(concat(
      [
        for x in jsondecode(aws_iam_role.this.assume_role_policy).Statement[0].Condition.StringEquals["token.actions.githubusercontent.com:sub"] :
        !strcontains(x, "*")
      ],
      [
        for x in jsondecode(aws_iam_role.this.assume_role_policy).Statement[0].Condition.StringEquals["token.actions.githubusercontent.com:job_workflow_ref"] :
        !strcontains(x, "*")
      ],
      [!strcontains(jsondecode(aws_iam_role.this.assume_role_policy).Statement[0].Condition.StringEquals["token.actions.githubusercontent.com:aud"], "*")],
    ))
    error_message = "No trust condition value may contain a wildcard."
  }

  assert {
    condition     = jsondecode(aws_iam_role.this.assume_role_policy).Statement[0].Condition.StringEquals["token.actions.githubusercontent.com:aud"] == "sts.amazonaws.com"
    error_message = "The audience must be pinned to sts.amazonaws.com."
  }
}

run "publish_and_deploy_profiles_are_disjoint" {
  command = plan

  assert {
    condition = length(setintersection(
      toset(var.permission_profiles["publish"]),
      toset(var.permission_profiles["deploy"])
    )) == 0
    error_message = "The publish and deploy profiles must share no action."
  }

  assert {
    condition = length(setintersection(
      toset(jsondecode(aws_iam_role_policy.this.policy).Statement[0].Action),
      toset(var.permission_profiles["deploy"])
    )) == 0
    error_message = "A publish role must be granted no deploy action."
  }
}

run "the_role_carries_exactly_one_profile" {
  command = plan

  assert {
    condition = length(setsubtract(
      toset(jsondecode(aws_iam_role_policy.this.policy).Statement[0].Action),
      toset(var.permission_profiles[var.permissions_profile])
    )) == 0
    error_message = "The role must grant only the actions of its own profile."
  }
}

run "rejects_overlapping_publish_and_deploy_profiles" {
  command = plan

  variables {
    permission_profiles = {
      publish  = ["s3:PutObject", "lambda:UpdateFunctionCode"]
      plan     = ["s3:GetObject"]
      deploy   = ["lambda:UpdateFunctionCode"]
      readonly = ["s3:GetObject"]
    }
  }

  expect_failures = [var.permission_profiles]
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
    permission_profiles = {
      publish  = ["s3:*"]
      plan     = ["s3:GetObject"]
      deploy   = ["lambda:UpdateFunctionCode"]
      readonly = ["s3:GetObject"]
    }
  }

  expect_failures = [var.permission_profiles]
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
