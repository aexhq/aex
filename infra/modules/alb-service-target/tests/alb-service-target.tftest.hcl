mock_provider "aws" {}

variables {
  name          = "aex-dev-euw1-stream"
  listener_arn  = "arn:aws:elasticloadbalancing:eu-west-1:000000000000:listener/app/aex-dev-euw1-public/50dc6c495c0c9188/f2f7dc8efc522ab2"
  vpc_id        = "vpc-0123456789abcdef0"
  target_port   = 8080
  priority      = 10
  path_patterns = ["/api/events/*"]
}

run "the_service_gets_exactly_one_rule_at_the_priority_it_asked_for" {
  command = plan

  assert {
    condition     = aws_lb_listener_rule.this.priority == var.priority
    error_message = "The rule must sit at the caller's explicit priority; that explicitness is what keeps two services from colliding."
  }

  assert {
    condition     = aws_lb_listener_rule.this.listener_arn == var.listener_arn
    error_message = "The rule must attach to the listener the caller named."
  }

  assert {
    condition     = one(aws_lb_listener_rule.this.action).type == "forward"
    error_message = "The rule must forward to this service's target group."
  }

  assert {
    condition     = length(one(aws_lb_listener_rule.this.condition).path_pattern) == 1
    error_message = "The rule must carry exactly one path-pattern condition."
  }
}

run "a_second_service_takes_a_different_priority_on_the_same_listener" {
  command = plan

  variables {
    name          = "aex-dev-euw1-session"
    priority      = 20
    path_patterns = ["/api/workspace/*"]
  }

  assert {
    condition     = aws_lb_listener_rule.this.priority == 20
    error_message = "A second service must be able to attach to the same listener at its own priority."
  }

  assert {
    condition     = aws_lb_target_group.this.name == "aex-dev-euw1-session-tg"
    error_message = "Each service gets its own target group, named after itself."
  }
}

run "the_rule_forwards_only_the_api_prefix" {
  command = plan

  variables {
    path_patterns = ["/api/events/*", "/api/logs/*"]
  }

  assert {
    condition = alltrue([
      for p in one(one(aws_lb_listener_rule.this.condition).path_pattern).values :
      startswith(p, "/api/")
    ])
    error_message = "The listener rule must forward only paths under /api/."
  }

  assert {
    condition = alltrue([
      for p in one(one(aws_lb_listener_rule.this.condition).path_pattern).values :
      !startswith(p, "/internal")
    ])
    error_message = "The listener rule must never forward /internal paths."
  }
}

run "the_health_check_probes_the_internal_readiness_path" {
  command = plan

  assert {
    condition     = one(aws_lb_target_group.this.health_check).path == "/internal/readyz"
    error_message = "The health check must probe /internal/readyz."
  }

  assert {
    condition     = tonumber(aws_lb_target_group.this.deregistration_delay) >= 30
    error_message = "The deregistration delay must be at least 30 seconds."
  }

  assert {
    condition     = aws_lb_target_group.this.target_type == "ip"
    error_message = "A Fargate service registers by address, so the target type must be ip."
  }
}

run "the_drain_window_is_reported_to_the_service_behind_it" {
  command = plan

  variables {
    deregistration_delay = 45
  }

  assert {
    condition     = output.deregistration_delay == 45
    error_message = "The module must report the drain window it configured, so the load balancer and the task cannot disagree."
  }

  assert {
    condition     = tonumber(aws_lb_target_group.this.deregistration_delay) == 45
    error_message = "The configured drain window must reach the target group."
  }
}

run "rejects_forwarding_an_internal_path" {
  command = plan

  variables {
    path_patterns = ["/internal/*"]
  }

  expect_failures = [var.path_patterns]
}

run "rejects_forwarding_everything" {
  command = plan

  variables {
    path_patterns = ["/*"]
  }

  expect_failures = [var.path_patterns]
}

run "rejects_a_pattern_outside_the_api_prefix" {
  command = plan

  variables {
    path_patterns = ["/api/sessions/*", "/healthz"]
  }

  expect_failures = [var.path_patterns]
}

run "rejects_an_empty_pattern_list" {
  command = plan

  variables {
    path_patterns = []
  }

  expect_failures = [var.path_patterns]
}

# `Condition Values per Rule` is a hard, non-adjustable AWS quota of 5. A split
# that needs more values than this does not fit one rule, and the failure has to
# land at plan rather than at apply.
run "rejects_more_path_patterns_than_one_rule_can_hold" {
  command = plan

  variables {
    path_patterns = [
      "/api/events/*",
      "/api/logs/*",
      "/api/metrics/*",
      "/api/spans/*",
      "/api/telemetry/*",
      "/api/traces/*",
    ]
  }

  expect_failures = [var.path_patterns]
}

# `Condition Wildcards per Rule` is a hard, non-adjustable AWS quota of 6.
run "rejects_more_wildcards_than_one_rule_can_hold" {
  command = plan

  variables {
    path_patterns = [
      "/api/sessions/*/events/*",
      "/api/sessions/*/logs/*",
      "/api/sessions/*/metrics/*",
      "/api/sessions/*/spans/*",
    ]
  }

  expect_failures = [var.path_patterns]
}

run "rejects_a_public_health_check_path" {
  command = plan

  variables {
    health_check_path = "/healthz"
  }

  expect_failures = [var.health_check_path]
}

run "rejects_a_relative_health_check_path" {
  command = plan

  variables {
    health_check_path = "internal/readyz"
  }

  expect_failures = [var.health_check_path]
}

run "rejects_a_deregistration_delay_below_thirty_seconds" {
  command = plan

  variables {
    deregistration_delay = 5
  }

  expect_failures = [var.deregistration_delay]
}

run "rejects_a_priority_outside_the_listener_range" {
  command = plan

  variables {
    priority = 0
  }

  expect_failures = [var.priority]
}

run "rejects_a_fractional_priority" {
  command = plan

  variables {
    priority = 10.5
  }

  expect_failures = [var.priority]
}

run "rejects_a_health_check_timeout_that_outlasts_its_interval" {
  command = plan

  variables {
    health_check = {
      interval            = 5
      timeout             = 15
      healthy_threshold   = 2
      unhealthy_threshold = 3
      matcher             = "200"
    }
  }

  expect_failures = [var.health_check]
}

run "rejects_a_target_group_name_that_would_be_truncated" {
  command = plan

  variables {
    name = "aex-dev-euw1-regional-session-api"
  }

  expect_failures = [var.name]
}
