mock_provider "aws" {
  mock_data "aws_s3_object" {
    defaults = {
      checksum_sha256 = "47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU="
    }
  }
}

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

  env = {
    AEX_PLANE  = "dev"
    AEX_REGION = "eu-west-1"
  }
}

run "code_comes_from_s3_and_the_function_publishes_a_version" {
  command = plan

  assert {
    condition     = aws_lambda_function.this.s3_bucket == var.artifact_bucket
    error_message = "Code must come from the artifact bucket."
  }

  assert {
    condition     = aws_lambda_function.this.s3_key == var.artifact_key
    error_message = "Code must come from the pinned artifact key."
  }

  assert {
    condition     = aws_lambda_function.this.s3_object_version == var.artifact_object_version
    error_message = "Code must come from the pinned object version."
  }

  assert {
    condition     = aws_lambda_function.this.publish == true
    error_message = "The function must publish a version so an alias can point at immutable bytes."
  }

  assert {
    condition     = aws_lambda_function.this.filename == null
    error_message = "A local filename would mean Terraform is packaging code; it must stay unset."
  }
}

run "the_stored_artifact_checksum_is_checked_before_deploy" {
  command = plan

  assert {
    condition     = data.aws_s3_object.artifact.checksum_sha256 == var.artifact_sha256
    error_message = "The precondition must compare the stored object checksum against the pinned digest."
  }
}

run "a_mismatched_artifact_checksum_fails_the_plan" {
  command = plan

  variables {
    artifact_sha256 = "3q2+7wAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
  }

  # The data-source postcondition is evaluated first and halts the walk, so the
  # function precondition guarding the same fact cannot also be observed here.
  expect_failures = [data.aws_s3_object.artifact]
}

run "the_alias_targets_the_published_version" {
  command = plan

  assert {
    condition     = aws_lambda_alias.this.name == var.alias_name
    error_message = "The alias must be created with the configured name."
  }

  assert {
    condition     = aws_lambda_alias.this.name != "$LATEST"
    error_message = "The alias must never be $LATEST."
  }
}

run "public_function_url_is_opt_in_and_alias_qualified" {
  command = plan

  variables {
    public_function_url_enabled = true
  }

  assert {
    condition     = one(aws_lambda_function_url.public).function_name == aws_lambda_function.this.function_name
    error_message = "The Function URL must target this module's function."
  }

  assert {
    condition     = one(aws_lambda_function_url.public).qualifier == aws_lambda_alias.this.name
    error_message = "The Function URL and its public policy must be scoped to the immutable alias."
  }

  assert {
    condition     = one(aws_lambda_function_url.public).authorization_type == "NONE" && one(aws_lambda_function_url.public).invoke_mode == "BUFFERED"
    error_message = "Webhook ingress must use the buffered Function URL contract and application-level authentication."
  }

  assert {
    condition     = length(one(aws_lambda_function_url.public).cors) == 0
    error_message = "A server-to-server webhook must not expose an unnecessary browser CORS policy."
  }
}

run "async_failures_use_the_alias_policy_and_unconsumed_dlq" {
  command = plan

  variables {
    async_failure_destination = {
      arn = "arn:aws:sqs:eu-west-1:000000000000:aex-dev-control-wake-dlq"
    }
  }

  assert {
    condition     = one(aws_lambda_function_event_invoke_config.this).qualifier == aws_lambda_alias.this.name
    error_message = "The asynchronous policy must follow the immutable live alias."
  }

  assert {
    condition     = one(one(aws_lambda_function_event_invoke_config.this).destination_config).on_failure[0].destination == var.async_failure_destination.arn
    error_message = "Failed asynchronous invocations must land in the configured alarmed DLQ."
  }
}

run "omitting_the_async_failure_destination_omits_the_policy" {
  command = plan

  assert {
    condition     = length(aws_lambda_function_event_invoke_config.this) == 0
    error_message = "An absent asynchronous failure destination must plan no invocation policy."
  }
}

run "rejects_an_invalid_async_failure_destination_arn" {
  command = plan

  variables {
    async_failure_destination = {
      arn = "not-an-sqs-queue-arn"
    }
  }

  expect_failures = [var.async_failure_destination]
}

run "rejects_an_environment_key_outside_the_aex_namespace" {
  command = plan

  variables {
    env = {
      DATABASE_URL = "postgres://localhost/aex"
    }
  }

  expect_failures = [var.env]
}

run "rejects_an_environment_value_that_looks_like_a_credential" {
  command = plan

  variables {
    env = {
      AEX_PROVIDER_KEY = "sk-not-a-real-key-000000000000"
    }
  }

  expect_failures = [var.env]
}

run "rejects_an_artifact_key_that_is_not_digest_addressed" {
  command = plan

  variables {
    artifact_key = "lambda/regional-session-api/latest.zip"
  }

  expect_failures = [var.artifact_key]
}

run "rejects_a_checksum_that_is_not_base64_sha256" {
  command = plan

  variables {
    artifact_sha256 = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
  }

  expect_failures = [var.artifact_sha256]
}

run "rejects_an_empty_object_version" {
  command = plan

  variables {
    artifact_object_version = ""
  }

  expect_failures = [var.artifact_object_version]
}
