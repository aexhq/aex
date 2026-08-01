mock_provider "aws" {}

variables {
  plane      = "dev"
  region     = "eu-west-1"
  account_id = "000000000000"

  state_bucket_suffix     = "0a1b2c3d"
  state_kms_alias         = "alias/aex-tfstate-dev"
  artifact_bucket_suffix  = "0a1b2c3d"
  artifact_retention_days = 90

  ops_topic_name = "aex-dev-ops"

  ops_publish_principals = [
    { type = "Service", identifier = "cloudwatch.amazonaws.com" },
    { type = "Service", identifier = "budgets.amazonaws.com" },
  ]

  ops_subscriptions = [
    { protocol = "https", endpoint = "https://alerts.example.invalid/aex-dev" },
  ]

  github_repository        = "example-owner/example-repo"
  github_oidc_provider_arn = "arn:aws:iam::000000000000:oidc-provider/token.actions.githubusercontent.com"
  github_allowed_refs      = ["refs/heads/main"]
  github_allowed_workflows = [".github/workflows/main.yml"]

  permission_profiles = {
    publish  = ["s3:PutObject", "ecr:PutImage", "ecr:InitiateLayerUpload", "ecr:UploadLayerPart", "ecr:CompleteLayerUpload"]
    plan     = ["s3:GetObject", "dynamodb:DescribeTable"]
    deploy   = ["lambda:UpdateFunctionCode", "lambda:PublishVersion", "ecs:UpdateService"]
    readonly = ["s3:GetObject"]
  }

  profile_resources = {
    publish  = ["arn:aws:s3:::aex-infra-artifacts-dev-0a1b2c3d/*"]
    plan     = ["arn:aws:s3:::aex-tfstate-dev-0a1b2c3d/*"]
    deploy   = ["arn:aws:lambda:eu-west-1:000000000000:function:aex-dev-regional-session-api"]
    readonly = ["arn:aws:s3:::aex-infra-artifacts-dev-0a1b2c3d/*"]
  }

  anomaly_thresholds = {
    absolute_usd = 50
    percentage   = 25
  }

  budgets = [
    {
      name              = "aex-dev-monthly"
      limit_amount      = 2000
      limit_unit        = "USD"
      time_unit         = "MONTHLY"
      threshold_percent = 80
      cost_filter_tags = {
        plane      = ["dev"]
        region     = ["eu-west-1"]
        releaseId  = ["any"]
        deployable = ["any"]
      }
    },
  ]
}

run "the_backbone_plans" {
  command = plan

  assert {
    condition     = module.tfstate.bucket == "aex-tfstate-dev-0a1b2c3d"
    error_message = "The state bucket name must be derived from the plane and suffix."
  }

  assert {
    condition     = module.artifacts.bucket == "aex-infra-artifacts-dev-0a1b2c3d"
    error_message = "The artifact bucket name must be derived from the plane and suffix."
  }
}

run "the_publish_role_carries_no_deploy_action" {
  command = plan

  assert {
    condition = length(setintersection(
      toset(var.permission_profiles["publish"]),
      toset(var.permission_profiles["deploy"])
    )) == 0
    error_message = "The publish and deploy profiles must stay disjoint at the root as well as in the module."
  }
}

run "the_mandatory_cost_tags_reach_the_budgets" {
  command = plan

  assert {
    condition = alltrue([
      for t in ["plane", "region", "releaseId", "deployable"] : contains(var.required_tags, t)
    ])
    error_message = "The mandatory cost allocation tag set must be complete."
  }
}
