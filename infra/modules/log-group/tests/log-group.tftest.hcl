mock_provider "aws" {}

variables {
  name           = "/aex/dev/session-stream-api"
  retention_days = 30
  kms_key_arn    = "arn:aws:kms:eu-west-1:000000000000:key/00000000-0000-4000-8000-000000000000"
}

run "the_group_is_retained_and_encrypted_with_the_customer_key" {
  command = plan

  assert {
    condition     = aws_cloudwatch_log_group.this.retention_in_days == 30
    error_message = "The group must carry the requested retention window."
  }

  assert {
    condition     = aws_cloudwatch_log_group.this.kms_key_id == var.kms_key_arn
    error_message = "The group must be encrypted with the supplied customer-managed key."
  }

  assert {
    condition     = aws_cloudwatch_log_group.this.name == var.name
    error_message = "The group must be created under the name the root composed."
  }
}

run "rejects_a_group_outside_the_plane_namespace" {
  command = plan

  variables {
    name = "/aws/lambda/aex-dev-central-authz"
  }

  expect_failures = [var.name]
}

run "rejects_unbounded_retention" {
  command = plan

  variables {
    retention_days = 0
  }

  expect_failures = [var.retention_days]
}

run "rejects_a_retention_window_cloudwatch_does_not_accept" {
  command = plan

  variables {
    retention_days = 45
  }

  expect_failures = [var.retention_days]
}

run "rejects_the_default_service_key" {
  command = plan

  variables {
    kms_key_arn = "alias/aws/logs"
  }

  expect_failures = [var.kms_key_arn]
}
