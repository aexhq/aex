mock_provider "aws" {
  mock_data "aws_s3_object" {
    defaults = {
      checksum_sha256 = "47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU="
    }
  }
}

run "produce_unknown_queue_arn" {
  command = plan

  module {
    source = "./tests/fixtures/unknown-queue-arn"
  }
}

run "an_enabled_destination_with_an_unknown_arn_still_plans_one_policy" {
  command = plan

  variables {
    function_name           = "aex-dev-regional-session-api"
    artifact_bucket         = "aex-infra-artifacts-dev-0a1b2c3d"
    artifact_key            = "lambda/regional-session-api/e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855.zip"
    artifact_object_version = "aBcDeFgHiJkLmNoPqRsTuVwXyZ012345"
    artifact_sha256         = "47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU="
    memory_mb               = 512
    timeout_s               = 30
    role_arn                = "arn:aws:iam::000000000000:role/aex-dev-regional-session-api"
    log_retention_days      = 30

    async_failure_destination = {
      arn = run.produce_unknown_queue_arn.queue_arn
    }
  }

  assert {
    condition     = length(aws_lambda_function_event_invoke_config.this) == 1
    error_message = "A present asynchronous failure destination object must plan one policy even when its producer ARN is unknown until apply."
  }
}

run "an_absent_destination_plans_no_policy" {
  command = plan

  variables {
    function_name           = "aex-dev-regional-session-api"
    artifact_bucket         = "aex-infra-artifacts-dev-0a1b2c3d"
    artifact_key            = "lambda/regional-session-api/e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855.zip"
    artifact_object_version = "aBcDeFgHiJkLmNoPqRsTuVwXyZ012345"
    artifact_sha256         = "47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU="
    memory_mb               = 512
    timeout_s               = 30
    role_arn                = "arn:aws:iam::000000000000:role/aex-dev-regional-session-api"
    log_retention_days      = 30

    async_failure_destination = null
  }

  assert {
    condition     = length(aws_lambda_function_event_invoke_config.this) == 0
    error_message = "An absent asynchronous failure destination object must plan no policy."
  }
}
