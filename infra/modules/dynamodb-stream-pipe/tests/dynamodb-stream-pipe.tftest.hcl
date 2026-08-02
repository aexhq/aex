mock_provider "aws" {}

variables {
  name              = "aex-dev-session-journal-hint"
  source_stream_arn = "arn:aws:dynamodb:eu-west-1:000000000000:table/aex-dev-euw1-session-journal/stream/2026-08-01T00:00:00.000"
  target_queue_arn  = "arn:aws:sqs:eu-west-1:000000000000:aex-dev-session-operation"
  filter_pattern    = "{\"eventName\":[\"INSERT\",\"MODIFY\"]}"
  input_template    = "{\"workId\": <$.dynamodb.NewImage.workId.S>}"
}

run "the_filter_pattern_is_non_empty_and_parses" {
  command = plan

  assert {
    condition     = length(trimspace(var.filter_pattern)) > 0
    error_message = "The pipe must carry a non-empty filter pattern."
  }

  assert {
    condition     = length(keys(jsondecode(var.filter_pattern))) > 0
    error_message = "The filter pattern must parse as a non-empty JSON object."
  }

  assert {
    condition     = one(one(one(aws_pipes_pipe.this.source_parameters).filter_criteria).filter).pattern == var.filter_pattern
    error_message = "The pipe must apply the supplied filter pattern verbatim."
  }

  assert {
    condition     = one(aws_pipes_pipe.this.target_parameters).input_template == var.input_template
    error_message = "The pipe must apply the supplied target input template verbatim."
  }
}

run "the_pipe_role_writes_only_to_the_target_queue" {
  command = plan

  assert {
    condition = alltrue(flatten([
      for s in jsondecode(aws_iam_role_policy.pipe.policy).Statement : [
        for a in s.Action : s.Resource == var.target_queue_arn if startswith(a, "sqs:")
      ]
    ]))
    error_message = "Every SQS grant in the pipe role must be scoped to the target queue."
  }

  assert {
    condition = length([
      for s in jsondecode(aws_iam_role_policy.pipe.policy).Statement : s
      if contains(s.Action, "sqs:SendMessage")
    ]) == 1
    error_message = "The pipe role must grant sqs:SendMessage exactly once."
  }

  assert {
    condition = alltrue(flatten([
      for s in jsondecode(aws_iam_role_policy.pipe.policy).Statement : [
        for a in s.Action : !contains(["sqs:*", "*"], a)
      ]
    ]))
    error_message = "The pipe role must not grant a wildcard SQS action."
  }
}

run "only_the_pipes_service_may_assume_the_role" {
  command = plan

  assert {
    condition = (
      length(jsondecode(aws_iam_role.pipe.assume_role_policy).Statement) == 1
      && length(jsondecode(aws_iam_role.pipe.assume_role_policy).Statement[0].Principal.Service) == 1
      && contains(jsondecode(aws_iam_role.pipe.assume_role_policy).Statement[0].Principal.Service, "pipes.amazonaws.com")
    )
    error_message = "Only the EventBridge Pipes service may assume the pipe role; the module grants write access to no other principal."
  }

  assert {
    condition = (
      jsondecode(aws_iam_role.pipe.assume_role_policy).Statement[0].Condition.StringEquals["aws:SourceAccount"] == "000000000000"
      && jsondecode(aws_iam_role.pipe.assume_role_policy).Statement[0].Condition.ArnEquals["aws:SourceArn"] == "arn:aws:pipes:eu-west-1:000000000000:pipe/${var.name}"
    )
    error_message = "The Pipes trust must be bound to this account and this exact pipe ARN."
  }
}

run "rejects_an_empty_input_template" {
  command = plan

  variables {
    input_template = "   "
  }

  expect_failures = [var.input_template]
}

run "list_streams_is_the_only_unscopable_source_action" {
  command = plan

  assert {
    condition = length([
      for s in jsondecode(aws_iam_role_policy.pipe.policy).Statement : s
      if contains(s.Action, "dynamodb:ListStreams") && s.Resource == "*"
    ]) == 1
    error_message = "dynamodb:ListStreams must use its required wildcard resource in one isolated statement."
  }

  assert {
    condition = alltrue(flatten([
      for s in jsondecode(aws_iam_role_policy.pipe.policy).Statement : [
        for a in s.Action : s.Resource == var.source_stream_arn
        if contains(["dynamodb:DescribeStream", "dynamodb:GetRecords", "dynamodb:GetShardIterator"], a)
      ]
    ]))
    error_message = "Every scopable DynamoDB Streams action must name only the source stream."
  }
}

run "rejects_an_empty_filter_pattern" {
  command = plan

  variables {
    filter_pattern = "   "
  }

  expect_failures = [var.filter_pattern]
}

run "rejects_a_filter_pattern_that_does_not_parse" {
  command = plan

  variables {
    filter_pattern = "eventName=INSERT"
  }

  expect_failures = [var.filter_pattern]
}

run "rejects_an_empty_filter_object" {
  command = plan

  variables {
    filter_pattern = "{}"
  }

  expect_failures = [var.filter_pattern]
}

run "rejects_a_target_that_is_not_a_queue" {
  command = plan

  variables {
    target_queue_arn = "arn:aws:sns:eu-west-1:000000000000:aex-dev-ops"
  }

  expect_failures = [var.target_queue_arn]
}
