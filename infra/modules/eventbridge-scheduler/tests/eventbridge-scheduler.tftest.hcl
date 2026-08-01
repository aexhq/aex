mock_provider "aws" {}

variables {
  group_name = "aex-dev-euw1"

  schedules = [
    {
      name                    = "content-lifecycle"
      description             = "Sweep expired content objects."
      expression              = "rate(1 hour)"
      flexible_window_minutes = 15
      target_arn              = "arn:aws:lambda:eu-west-1:000000000000:function:aex-dev-content-lifecycle-worker:live"
      role_arn                = "arn:aws:iam::000000000000:role/aex-dev-scheduler"
      input                   = "{\"sweep\":\"expired\"}"
      dead_letter_arn         = "arn:aws:sqs:eu-west-1:000000000000:aex-dev-scheduler-dlq"
    },
    {
      name                    = "observation-export"
      description             = "Launch the observation export task."
      expression              = "cron(0 2 * * ? *)"
      flexible_window_minutes = 0
      target_arn              = "arn:aws:ecs:eu-west-1:000000000000:task-definition/aex-dev-observation-export-task:7"
      role_arn                = "arn:aws:iam::000000000000:role/aex-dev-scheduler"
      input                   = "{\"window\":\"daily\"}"
    },
  ]
}

run "every_target_is_immutable" {
  command = plan

  assert {
    condition = alltrue([
      for k, s in aws_scheduler_schedule.this :
      can(regex(":function:[A-Za-z0-9_-]+:[A-Za-z0-9_-]+$", one(s.target).arn))
      || can(regex(":task-definition/[A-Za-z0-9_-]+:[0-9]+$", one(s.target).arn))
    ])
    error_message = "Every schedule target must be a Lambda alias or a revisioned task definition."
  }

  assert {
    condition = alltrue([
      for k, s in aws_scheduler_schedule.this : !endswith(one(s.target).arn, ":$LATEST")
    ])
    error_message = "No schedule may target $LATEST."
  }
}

run "schedules_are_created_in_the_group_and_enabled" {
  command = plan

  assert {
    condition = alltrue([
      for k, s in aws_scheduler_schedule.this : s.group_name == var.group_name
    ])
    error_message = "Every schedule must be created in the configured group."
  }

  assert {
    condition = alltrue([
      for k, s in aws_scheduler_schedule.this : s.state == "ENABLED"
    ])
    error_message = "Every schedule must be enabled."
  }
}

run "the_flexible_window_is_off_when_zero" {
  command = plan

  assert {
    condition     = one(aws_scheduler_schedule.this["observation-export"].flexible_time_window).mode == "OFF"
    error_message = "A zero flexible window must switch the mode off rather than request a zero-minute window."
  }

  assert {
    condition     = one(aws_scheduler_schedule.this["content-lifecycle"].flexible_time_window).maximum_window_in_minutes == 15
    error_message = "A non-zero flexible window must be applied."
  }
}

run "rejects_an_unqualified_lambda_target" {
  command = plan

  variables {
    schedules = [
      {
        name                    = "content-lifecycle"
        description             = "Sweep expired content objects."
        expression              = "rate(1 hour)"
        flexible_window_minutes = 15
        target_arn              = "arn:aws:lambda:eu-west-1:000000000000:function:aex-dev-content-lifecycle-worker"
        role_arn                = "arn:aws:iam::000000000000:role/aex-dev-scheduler"
        input                   = "{}"
      },
    ]
  }

  expect_failures = [var.schedules]
}

run "rejects_a_task_definition_with_no_revision" {
  command = plan

  variables {
    schedules = [
      {
        name                    = "observation-export"
        description             = "Launch the observation export task."
        expression              = "cron(0 2 * * ? *)"
        flexible_window_minutes = 0
        target_arn              = "arn:aws:ecs:eu-west-1:000000000000:task-definition/aex-dev-observation-export-task"
        role_arn                = "arn:aws:iam::000000000000:role/aex-dev-scheduler"
        input                   = "{}"
      },
    ]
  }

  expect_failures = [var.schedules]
}

run "rejects_a_latest_qualifier" {
  command = plan

  variables {
    schedules = [
      {
        name                    = "content-lifecycle"
        description             = "Sweep expired content objects."
        expression              = "rate(1 hour)"
        flexible_window_minutes = 15
        target_arn              = "arn:aws:lambda:eu-west-1:000000000000:function:aex-dev-content-lifecycle-worker:$LATEST"
        role_arn                = "arn:aws:iam::000000000000:role/aex-dev-scheduler"
        input                   = "{}"
      },
    ]
  }

  expect_failures = [var.schedules]
}

run "rejects_an_invalid_expression" {
  command = plan

  variables {
    schedules = [
      {
        name                    = "content-lifecycle"
        description             = "Sweep expired content objects."
        expression              = "hourly"
        flexible_window_minutes = 15
        target_arn              = "arn:aws:lambda:eu-west-1:000000000000:function:aex-dev-content-lifecycle-worker:live"
        role_arn                = "arn:aws:iam::000000000000:role/aex-dev-scheduler"
        input                   = "{}"
      },
    ]
  }

  expect_failures = [var.schedules]
}

run "rejects_an_input_that_is_not_json" {
  command = plan

  variables {
    schedules = [
      {
        name                    = "content-lifecycle"
        description             = "Sweep expired content objects."
        expression              = "rate(1 hour)"
        flexible_window_minutes = 15
        target_arn              = "arn:aws:lambda:eu-west-1:000000000000:function:aex-dev-content-lifecycle-worker:live"
        role_arn                = "arn:aws:iam::000000000000:role/aex-dev-scheduler"
        input                   = "sweep=expired"
      },
    ]
  }

  expect_failures = [var.schedules]
}
