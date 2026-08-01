mock_provider "aws" {}

variables {
  name               = "session-operation"
  visibility_timeout = 300
  max_receive_count  = 5
  kms_key_arn        = "arn:aws:kms:eu-west-1:000000000000:key/00000000-0000-4000-8000-000000000000"

  dlq = {
    enabled                   = true
    message_retention_seconds = 1209600
  }

  policy_statements = [
    {
      sid            = "AllowProducer"
      effect         = "Allow"
      principal_type = "AWS"
      principals     = ["arn:aws:iam::000000000000:role/aex-session-operation-producer"]
      actions        = ["sqs:SendMessage"]
    },
  ]
}

run "encryption_is_a_customer_managed_key_and_a_dead_letter_queue_exists" {
  command = plan

  assert {
    condition     = aws_sqs_queue.this.kms_master_key_id == var.kms_key_arn
    error_message = "The main queue must be encrypted with the supplied customer-managed key."
  }

  assert {
    condition     = length(aws_sqs_queue.this.kms_master_key_id) > 0
    error_message = "SQS-managed encryption is not acceptable; a customer-managed key must always be set."
  }

  assert {
    condition     = aws_sqs_queue.dlq.kms_master_key_id == var.kms_key_arn
    error_message = "The dead-letter queue must be encrypted with the same customer-managed key."
  }

  assert {
    condition     = aws_sqs_queue.dlq.name == "${var.name}-dlq"
    error_message = "A dead-letter queue must always be created alongside the main queue."
  }
}

run "standard_queue_has_no_fifo_suffix" {
  command = plan

  assert {
    condition     = aws_sqs_queue.this.name == var.name
    error_message = "A standard queue must not carry the .fifo suffix."
  }

  assert {
    condition     = aws_sqs_queue.this.fifo_queue == false
    error_message = "A standard queue must not be marked FIFO."
  }
}

run "fifo_queue_is_supported_and_suffixed" {
  command = plan

  variables {
    name                = "usage-rating"
    fifo                = true
    content_based_dedup = true
  }

  assert {
    condition     = aws_sqs_queue.this.name == "usage-rating.fifo"
    error_message = "A FIFO queue name must carry the .fifo suffix."
  }

  assert {
    condition     = aws_sqs_queue.dlq.name == "usage-rating-dlq.fifo"
    error_message = "A FIFO dead-letter queue name must carry the .fifo suffix."
  }

  assert {
    condition     = aws_sqs_queue.this.deduplication_scope == "messageGroup"
    error_message = "FIFO deduplication must be scoped per message group."
  }
}

run "queue_policy_has_no_wildcard_principal" {
  command = apply

  assert {
    condition = alltrue([
      for s in jsondecode(aws_sqs_queue_policy.this[0].policy).Statement :
      !contains(values(s.Principal)[0], "*")
    ])
    error_message = "The rendered queue policy must not contain a wildcard principal."
  }

  assert {
    condition = alltrue([
      for s in jsondecode(aws_sqs_queue_policy.this[0].policy).Statement :
      s.Resource == aws_sqs_queue.this.arn
    ])
    error_message = "Every queue policy statement must be scoped to this queue only."
  }
}

run "rejects_a_usage_rating_queue_that_is_not_fifo" {
  command = plan

  variables {
    name = "usage-rating-compute"
    fifo = false
  }

  expect_failures = [var.fifo]
}

run "rejects_disabling_the_dead_letter_queue" {
  command = plan

  variables {
    dlq = {
      enabled                   = false
      message_retention_seconds = 1209600
    }
  }

  expect_failures = [var.dlq]
}

run "rejects_an_sqs_managed_encryption_placeholder" {
  command = plan

  variables {
    kms_key_arn = "alias/aws/sqs"
  }

  expect_failures = [var.kms_key_arn]
}

run "rejects_a_wildcard_principal_in_the_queue_policy" {
  command = plan

  variables {
    policy_statements = [
      {
        sid            = "AnyPrincipal"
        effect         = "Allow"
        principal_type = "AWS"
        principals     = ["*"]
        actions        = ["sqs:SendMessage"]
      },
    ]
  }

  expect_failures = [var.policy_statements]
}

run "rejects_a_wildcard_action_in_the_queue_policy" {
  command = plan

  variables {
    policy_statements = [
      {
        sid            = "AnyAction"
        effect         = "Allow"
        principal_type = "AWS"
        principals     = ["arn:aws:iam::000000000000:role/aex-session-operation-producer"]
        actions        = ["sqs:*"]
      },
    ]
  }

  expect_failures = [var.policy_statements]
}
