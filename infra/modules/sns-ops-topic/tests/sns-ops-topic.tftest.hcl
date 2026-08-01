mock_provider "aws" {}

variables {
  name       = "aex-dev-ops"
  region     = "eu-west-1"
  account_id = "000000000000"

  publish_principals = [
    { type = "Service", identifier = "cloudwatch.amazonaws.com" },
    { type = "AWS", identifier = "arn:aws:iam::000000000000:role/aex-dev-observation-reconciler" },
  ]

  subscriptions = [
    { protocol = "https", endpoint = "https://alerts.example.invalid/aex-dev" },
  ]
}

run "the_topic_policy_has_no_wildcard_publish_principal" {
  command = plan

  assert {
    condition = alltrue([
      for s in jsondecode(aws_sns_topic_policy.this.policy).Statement :
      !strcontains(values(s.Principal)[0], "*")
    ])
    error_message = "No publish principal may be a wildcard."
  }

  assert {
    condition = alltrue([
      for s in jsondecode(aws_sns_topic_policy.this.policy).Statement :
      contains(s.Action, "sns:Publish") && length(s.Action) == 1
    ])
    error_message = "A publish grant must grant publish and nothing else."
  }

  assert {
    condition = alltrue([
      for s in jsondecode(aws_sns_topic_policy.this.policy).Statement :
      s.Resource == "arn:aws:sns:eu-west-1:000000000000:aex-dev-ops"
    ])
    error_message = "Every statement must be scoped to this topic."
  }
}

run "subscriptions_are_created" {
  command = plan

  assert {
    condition     = length(aws_sns_topic_subscription.this) == 1
    error_message = "The configured subscription must be created."
  }

  assert {
    condition = alltrue([
      for k, s in aws_sns_topic_subscription.this : s.protocol == "https"
    ])
    error_message = "The subscription protocol must be applied."
  }
}

run "rejects_a_wildcard_publish_principal" {
  command = plan

  variables {
    publish_principals = [
      { type = "AWS", identifier = "*" },
    ]
  }

  expect_failures = [var.publish_principals]
}

run "rejects_a_partial_wildcard_publish_principal" {
  command = plan

  variables {
    publish_principals = [
      { type = "AWS", identifier = "arn:aws:iam::000000000000:role/*" },
    ]
  }

  expect_failures = [var.publish_principals]
}

run "rejects_an_empty_publish_principal_list" {
  command = plan

  variables {
    publish_principals = []
  }

  expect_failures = [var.publish_principals]
}

run "rejects_an_https_subscription_that_is_not_https" {
  command = plan

  variables {
    subscriptions = [
      { protocol = "https", endpoint = "http://alerts.example.invalid/aex-dev" },
    ]
  }

  expect_failures = [var.subscriptions]
}
