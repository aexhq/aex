mock_provider "aws" {}

variables {
  plane         = "dev"
  region        = "eu-west-1"
  ops_topic_arn = "arn:aws:sns:eu-west-1:000000000000:aex-dev-ops"

  artifact_key = {
    alias                     = "alias/aex-artifacts-dev"
    description               = "Published artifact encryption for the dev plane."
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
        sid            = "RegistryUse"
        effect         = "Allow"
        principal_type = "Service"
        principals     = ["ecr.amazonaws.com"]
        actions        = ["kms:Encrypt", "kms:Decrypt", "kms:GenerateDataKey"]
        resources      = ["*"]
        data_plane     = false
      },
    ]
  }

  repositories = {
    "aex/brain-mux"       = { untagged_expire_days = 14 }
    "aex/regional-stream" = { untagged_expire_days = 14 }
  }

  github_repository        = "example-owner/example-repo"
  github_oidc_provider_arn = "arn:aws:iam::000000000000:oidc-provider/token.actions.githubusercontent.com"
  github_allowed_refs      = ["refs/heads/main"]
  github_allowed_workflows = [".github/workflows/release.yml"]

  permission_profiles = {
    publish  = ["s3:PutObject", "ecr:PutImage"]
    plan     = ["s3:GetObject"]
    deploy   = ["lambda:UpdateFunctionCode", "lambda:PublishVersion", "ecs:UpdateService"]
    readonly = ["s3:GetObject"]
  }

  profile_resources = {
    publish  = ["arn:aws:s3:::aex-infra-artifacts-dev-0a1b2c3d/*"]
    plan     = ["arn:aws:s3:::aex-tfstate-dev-0a1b2c3d/*"]
    deploy   = ["arn:aws:lambda:eu-west-1:000000000000:function:aex-dev-regional-session-api"]
    readonly = ["arn:aws:s3:::aex-infra-artifacts-dev-0a1b2c3d/*"]
  }

  alarm_specs = [
    {
      name                = "aex-dev-deploy-rollback"
      description         = "A deployment tripped the circuit breaker."
      owner               = "delivery"
      urgency             = "page"
      runbook_url         = "https://runbooks.example.invalid/deploy-rollback"
      action_class        = "notify"
      category            = "correctness"
      namespace           = "AEX/Delivery"
      metric_name         = "DeploymentRollbackCount"
      statistic           = "Sum"
      period              = 300
      evaluation_periods  = 1
      threshold           = 0
      comparison_operator = "GreaterThanThreshold"
      treat_missing_data  = "notBreaching"
    },
  ]
}

run "the_plane_foundation_plans" {
  command = plan

  assert {
    condition     = length(module.repository) == 2
    error_message = "Every declared repository must be created."
  }
}

run "every_repository_is_immutable" {
  command = plan

  assert {
    condition = alltrue([
      for k, v in var.repositories : v.untagged_expire_days >= 1
    ])
    error_message = "Every repository must declare an untagged expiry window."
  }
}

run "the_deploy_role_carries_no_publish_action" {
  command = plan

  assert {
    condition = length(setintersection(
      toset(var.permission_profiles["publish"]),
      toset(var.permission_profiles["deploy"])
    )) == 0
    error_message = "The publish and deploy profiles must stay disjoint."
  }
}
