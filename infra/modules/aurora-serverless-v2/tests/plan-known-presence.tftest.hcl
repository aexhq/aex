mock_provider "aws" {}

run "produce_unknown_role_arn" {
  command = plan

  module {
    source = "./tests/fixtures/unknown-role-arn"
  }
}

run "an_enabled_role_with_an_unknown_arn_still_plans_one_association" {
  command = plan

  variables {
    cluster_identifier     = "aex-dev-central-finance"
    database_name          = "aex_finance"
    master_username        = "aex_admin"
    engine_version         = "17.5"
    subnet_ids             = ["subnet-0123456789abcdef0", "subnet-0123456789abcdef1"]
    vpc_security_group_ids = ["sg-0123456789abcdef0"]
    region                 = "eu-west-1"
    kms_key_arn            = "arn:aws:kms:eu-west-1:000000000000:key/00000000-0000-4000-8000-000000000000"

    final_snapshot_identifier = "aex-dev-central-finance-final"
    lambda_invoke_role = {
      arn = run.produce_unknown_role_arn.role_arn
    }
  }

  assert {
    condition     = length(aws_rds_cluster_role_association.lambda_invoke) == 1
    error_message = "A present Lambda invocation role object must plan one association even when its producer ARN is unknown until apply."
  }
}

run "an_absent_role_plans_no_association" {
  command = plan

  variables {
    cluster_identifier     = "aex-dev-central-finance"
    database_name          = "aex_finance"
    master_username        = "aex_admin"
    engine_version         = "17.5"
    subnet_ids             = ["subnet-0123456789abcdef0", "subnet-0123456789abcdef1"]
    vpc_security_group_ids = ["sg-0123456789abcdef0"]
    region                 = "eu-west-1"
    kms_key_arn            = "arn:aws:kms:eu-west-1:000000000000:key/00000000-0000-4000-8000-000000000000"

    final_snapshot_identifier = "aex-dev-central-finance-final"
    lambda_invoke_role        = null
  }

  assert {
    condition     = length(aws_rds_cluster_role_association.lambda_invoke) == 0
    error_message = "An absent Lambda invocation role object must plan no association."
  }
}
