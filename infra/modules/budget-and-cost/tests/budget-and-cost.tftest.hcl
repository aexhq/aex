mock_provider "aws" {}

variables {
  sns_topic_arn = "arn:aws:sns:eu-west-1:000000000000:aex-dev-ops"
  required_tags = ["plane", "region", "releaseId", "deployable"]

  anomaly_thresholds = {
    absolute_usd = 50
    percentage   = 25
  }

  budgets = [
    {
      name              = "aex-dev-euw1-monthly"
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

run "the_mandatory_tag_set_is_complete" {
  command = plan

  assert {
    condition = alltrue([
      for t in ["plane", "region", "releaseId", "deployable"] : contains(var.required_tags, t)
    ])
    error_message = "The mandatory tag set must include plane, region, releaseId and deployable."
  }

  assert {
    condition = alltrue([
      for t in ["plane", "region", "releaseId", "deployable"] : contains(output.required_tags, t)
    ])
    error_message = "The module must report the mandatory tag set to its callers."
  }
}

run "every_budget_filters_on_the_mandatory_tags_and_notifies_the_topic" {
  command = plan

  assert {
    condition = alltrue([
      for k, b in aws_budgets_budget.this : length(b.cost_filter) == 4
    ])
    error_message = "Every budget must carry one cost filter per mandatory tag."
  }

  assert {
    condition = alltrue(flatten([
      for k, b in aws_budgets_budget.this : [
        for n in b.notification : contains(n.subscriber_sns_topic_arns, var.sns_topic_arn)
      ]
    ]))
    error_message = "Every budget notification must publish to the ops topic."
  }

  assert {
    condition = alltrue([
      for k, b in aws_budgets_budget.this : length(b.notification) == 2
    ])
    error_message = "Every budget must notify on both actual and forecasted spend."
  }
}

run "anomaly_detection_uses_both_thresholds" {
  command = plan

  assert {
    condition     = length(one(aws_ce_anomaly_subscription.this.threshold_expression).or) == 2
    error_message = "Anomaly detection must fire on either an absolute or a percentage threshold."
  }

  assert {
    condition = alltrue([
      for s in aws_ce_anomaly_subscription.this.subscriber : s.address == var.sns_topic_arn
    ])
    error_message = "The anomaly subscription must publish to the ops topic."
  }
}

run "rejects_a_tag_set_missing_the_release_id" {
  command = plan

  variables {
    required_tags = ["plane", "region", "deployable"]
  }

  expect_failures = [var.required_tags]
}

run "rejects_a_budget_that_does_not_filter_on_every_mandatory_tag" {
  command = plan

  variables {
    budgets = [
      {
        name              = "aex-dev-euw1-monthly"
        limit_amount      = 2000
        limit_unit        = "USD"
        time_unit         = "MONTHLY"
        threshold_percent = 80
        cost_filter_tags = {
          plane = ["dev"]
        }
      },
    ]
  }

  expect_failures = [var.budgets]
}

run "rejects_a_zero_budget" {
  command = plan

  variables {
    budgets = [
      {
        name              = "aex-dev-euw1-monthly"
        limit_amount      = 0
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

  expect_failures = [var.budgets]
}

run "rejects_a_non_positive_anomaly_threshold" {
  command = plan

  variables {
    anomaly_thresholds = {
      absolute_usd = 0
      percentage   = 25
    }
  }

  expect_failures = [var.anomaly_thresholds]
}
