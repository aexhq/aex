mock_provider "aws" {}

variables {
  function_alias_arn = "arn:aws:lambda:eu-west-1:000000000000:function:aex-dev-session-operation-worker:live"
  source_arn         = "arn:aws:sqs:eu-west-1:000000000000:aex-dev-session-operation"
  batch_size         = 10
}

run "partial_batch_failures_are_always_reported" {
  command = plan

  assert {
    condition = (
      contains(aws_lambda_event_source_mapping.this.function_response_types, "ReportBatchItemFailures")
      && length(aws_lambda_event_source_mapping.this.function_response_types) == 1
    )
    error_message = "The mapping must request ReportBatchItemFailures."
  }
}

run "the_target_is_an_alias" {
  command = plan

  assert {
    condition     = aws_lambda_event_source_mapping.this.function_name == var.function_alias_arn
    error_message = "The mapping must target the alias ARN it was given."
  }

  assert {
    condition     = !endswith(aws_lambda_event_source_mapping.this.function_name, ":$LATEST")
    error_message = "The mapping must never target $LATEST."
  }

  assert {
    condition     = length(split(":", aws_lambda_event_source_mapping.this.function_name)) == 8
    error_message = "The mapping target must be a qualified alias ARN, not an unqualified function ARN."
  }
}

run "a_queue_source_may_carry_a_scaling_config" {
  command = plan

  variables {
    scaling_config = { maximum_concurrency = 20 }
  }

  assert {
    condition     = one(aws_lambda_event_source_mapping.this.scaling_config).maximum_concurrency == 20
    error_message = "The queue concurrency ceiling must be applied."
  }
}

run "a_stream_source_starts_from_a_declared_position" {
  command = plan

  variables {
    source_arn        = "arn:aws:dynamodb:eu-west-1:000000000000:table/aex-dev-euw1-session-journal/stream/2026-08-01T00:00:00.000"
    starting_position = "TRIM_HORIZON"
  }

  assert {
    condition     = aws_lambda_event_source_mapping.this.starting_position == "TRIM_HORIZON"
    error_message = "A stream mapping must start from the declared position."
  }
}

run "rejects_disabling_partial_batch_responses" {
  command = plan

  variables {
    partial_batch_response = false
  }

  expect_failures = [var.partial_batch_response]
}

run "rejects_an_unqualified_function_arn" {
  command = plan

  variables {
    function_alias_arn = "arn:aws:lambda:eu-west-1:000000000000:function:aex-dev-session-operation-worker"
  }

  expect_failures = [var.function_alias_arn]
}

run "rejects_a_latest_qualifier" {
  command = plan

  variables {
    function_alias_arn = "arn:aws:lambda:eu-west-1:000000000000:function:aex-dev-session-operation-worker:$LATEST"
  }

  expect_failures = [var.function_alias_arn]
}

run "rejects_a_stream_source_with_no_starting_position" {
  command = plan

  variables {
    source_arn = "arn:aws:dynamodb:eu-west-1:000000000000:table/aex-dev-euw1-session-journal/stream/2026-08-01T00:00:00.000"
  }

  expect_failures = [var.starting_position]
}

run "rejects_a_scaling_config_on_a_stream_source" {
  command = plan

  variables {
    source_arn        = "arn:aws:dynamodb:eu-west-1:000000000000:table/aex-dev-euw1-session-journal/stream/2026-08-01T00:00:00.000"
    starting_position = "LATEST"
    scaling_config    = { maximum_concurrency = 20 }
  }

  expect_failures = [var.scaling_config]
}
