mock_provider "aws" {}

variables {
  sns_topic_arn = "arn:aws:sns:eu-west-1:000000000000:aex-dev-ops"

  alarm_specs = [
    {
      name                = "aex-dev-settlement-imbalance"
      description         = "A settlement batch did not balance."
      owner               = "central-finance"
      urgency             = "page"
      runbook_url         = "https://runbooks.example.invalid/settlement-imbalance"
      action_class        = "none"
      category            = "correctness"
      namespace           = "AEX/Finance"
      metric_name         = "SettlementImbalanceCount"
      statistic           = "Sum"
      period              = 300
      evaluation_periods  = 1
      threshold           = 0
      comparison_operator = "GreaterThanThreshold"
      treat_missing_data  = "notBreaching"
    },
    {
      name                = "aex-dev-observation-export-silence"
      description         = "No observation export heartbeat."
      owner               = "observations-usage"
      urgency             = "ticket"
      runbook_url         = "https://runbooks.example.invalid/observation-export-silence"
      action_class        = "notify"
      category            = "telemetry_loss"
      namespace           = "AEX/Observations"
      metric_name         = "ExportHeartbeat"
      statistic           = "SampleCount"
      period              = 900
      evaluation_periods  = 2
      threshold           = 1
      comparison_operator = "LessThanThreshold"
      treat_missing_data  = "breaching"
    },
  ]
}

run "every_alarm_has_an_owner_and_a_runbook" {
  command = plan

  assert {
    condition = alltrue([
      for k, a in aws_cloudwatch_metric_alarm.this : length(a.tags["aex:alarm-owner"]) > 0
    ])
    error_message = "Every alarm must carry an owner tag."
  }

  assert {
    condition = alltrue([
      for k, a in aws_cloudwatch_metric_alarm.this : startswith(a.tags["aex:runbook"], "https://")
    ])
    error_message = "Every alarm must carry an https runbook URL."
  }

  assert {
    condition = alltrue([
      for k, a in aws_cloudwatch_metric_alarm.this : strcontains(a.alarm_description, "Runbook: https://")
    ])
    error_message = "The runbook URL must also appear in the alarm description an operator sees first."
  }
}

run "correctness_alarms_are_tagged_apart_from_telemetry_loss_alarms" {
  command = plan

  assert {
    condition     = aws_cloudwatch_metric_alarm.this["aex-dev-settlement-imbalance"].tags["aex:alarm-category"] == "correctness"
    error_message = "A correctness alarm must be tagged as such."
  }

  assert {
    condition     = aws_cloudwatch_metric_alarm.this["aex-dev-observation-export-silence"].tags["aex:alarm-category"] == "telemetry_loss"
    error_message = "A telemetry-loss alarm must be tagged as such."
  }

  assert {
    condition = (
      aws_cloudwatch_metric_alarm.this["aex-dev-settlement-imbalance"].tags["aex:alarm-category"]
      != aws_cloudwatch_metric_alarm.this["aex-dev-observation-export-silence"].tags["aex:alarm-category"]
    )
    error_message = "Correctness and telemetry-loss alarms must never share a category tag; they demand opposite responses."
  }

  assert {
    condition     = length(aws_cloudwatch_metric_alarm.this["aex-dev-observation-export-silence"].insufficient_data_actions) == 1
    error_message = "A telemetry-loss alarm must notify on insufficient data; silence is the failure it exists to catch."
  }

  assert {
    condition     = length(aws_cloudwatch_metric_alarm.this["aex-dev-settlement-imbalance"].insufficient_data_actions) == 0
    error_message = "A correctness alarm must not page on insufficient data."
  }
}

run "rejects_an_alarm_with_no_runbook" {
  command = plan

  variables {
    alarm_specs = [
      {
        name                = "aex-dev-settlement-imbalance"
        description         = "A settlement batch did not balance."
        owner               = "central-finance"
        urgency             = "page"
        runbook_url         = ""
        action_class        = "none"
        category            = "correctness"
        namespace           = "AEX/Finance"
        metric_name         = "SettlementImbalanceCount"
        statistic           = "Sum"
        period              = 300
        evaluation_periods  = 1
        threshold           = 0
        comparison_operator = "GreaterThanThreshold"
        treat_missing_data  = "notBreaching"
      },
    ]
  }

  expect_failures = [var.alarm_specs]
}

run "rejects_an_alarm_with_an_unknown_owner" {
  command = plan

  variables {
    alarm_specs = [
      {
        name                = "aex-dev-settlement-imbalance"
        description         = "A settlement batch did not balance."
        owner               = "someone"
        urgency             = "page"
        runbook_url         = "https://runbooks.example.invalid/settlement-imbalance"
        action_class        = "none"
        category            = "correctness"
        namespace           = "AEX/Finance"
        metric_name         = "SettlementImbalanceCount"
        statistic           = "Sum"
        period              = 300
        evaluation_periods  = 1
        threshold           = 0
        comparison_operator = "GreaterThanThreshold"
        treat_missing_data  = "notBreaching"
      },
    ]
  }

  expect_failures = [var.alarm_specs]
}

run "rejects_a_telemetry_loss_alarm_that_ignores_silence" {
  command = plan

  variables {
    alarm_specs = [
      {
        name                = "aex-dev-observation-export-silence"
        description         = "No observation export heartbeat."
        owner               = "observations-usage"
        urgency             = "ticket"
        runbook_url         = "https://runbooks.example.invalid/observation-export-silence"
        action_class        = "notify"
        category            = "telemetry_loss"
        namespace           = "AEX/Observations"
        metric_name         = "ExportHeartbeat"
        statistic           = "SampleCount"
        period              = 900
        evaluation_periods  = 2
        threshold           = 1
        comparison_operator = "LessThanThreshold"
        treat_missing_data  = "notBreaching"
      },
    ]
  }

  expect_failures = [var.alarm_specs]
}

run "rejects_an_unknown_category" {
  command = plan

  variables {
    alarm_specs = [
      {
        name                = "aex-dev-settlement-imbalance"
        description         = "A settlement batch did not balance."
        owner               = "central-finance"
        urgency             = "page"
        runbook_url         = "https://runbooks.example.invalid/settlement-imbalance"
        action_class        = "none"
        category            = "whatever"
        namespace           = "AEX/Finance"
        metric_name         = "SettlementImbalanceCount"
        statistic           = "Sum"
        period              = 300
        evaluation_periods  = 1
        threshold           = 0
        comparison_operator = "GreaterThanThreshold"
        treat_missing_data  = "notBreaching"
      },
    ]
  }

  expect_failures = [var.alarm_specs]
}
