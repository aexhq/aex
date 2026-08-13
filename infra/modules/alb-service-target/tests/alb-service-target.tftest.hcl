mock_provider "aws" {}

variables {
  name         = "aex-dev-euw1-session"
  listener_arn = "arn:aws:elasticloadbalancing:eu-west-1:000000000000:listener/app/aex-dev-euw1-public/50dc6c495c0c9188/f2f7dc8efc522ab2"
  vpc_id       = "vpc-0123456789abcdef0"
  target_port  = 8080

  rules = [
    {
      priority      = 10
      path_patterns = ["/api/sessions/*"]
    },
  ]
}

run "the_service_gets_a_rule_at_the_priority_it_asked_for" {
  command = plan

  assert {
    condition     = aws_lb_listener_rule.this["10"].priority == 10
    error_message = "The rule must sit at the caller's explicit priority; that explicitness is what keeps two services from colliding."
  }

  assert {
    condition     = aws_lb_listener_rule.this["10"].listener_arn == var.listener_arn
    error_message = "The rule must attach to the listener the caller named."
  }

  assert {
    condition     = one(aws_lb_listener_rule.this["10"].action).type == "forward"
    error_message = "The rule must forward to this service's target group."
  }

  assert {
    condition     = length(aws_lb_listener_rule.this) == 1
    error_message = "A single-element rule list must create exactly one rule."
  }
}

# The quotas bite per rule, not per service: `Condition Values per Rule` is 5
# and `Condition Wildcards per Rule` is 6, both fixed, while `Rules per
# Application Load Balancer` is 100 and adjustable. A surface too wide for one
# rule is expressed as several rules against the one target group.
run "one_target_group_can_carry_several_rules" {
  command = plan

  variables {
    rules = [
      {
        priority      = 10
        path_patterns = ["/api/example-a/*", "/api/example-b/*", "/api/example-c/*", "/api/example-d/*", "/api/example-e/*"]
      },
      {
        priority      = 11
        path_patterns = ["/api/example-f/*"]
      },
      {
        priority      = 12
        path_patterns = ["/api/examples/*/events/*", "/api/examples/*/logs/*", "/api/examples/*/metrics/*"]
      },
    ]
  }

  assert {
    condition     = length(aws_lb_listener_rule.this) == 3
    error_message = "Each element of the rule list must become its own listener rule."
  }

  assert {
    condition = alltrue([
      for key, rule in aws_lb_listener_rule.this :
      one(rule.action).type == "forward"
    ])
    error_message = "Every rule must forward to the one target group this module creates; the module is one service's whole attachment."
  }

  assert {
    condition     = aws_lb_target_group.this.name == "aex-dev-euw1-session-tg"
    error_message = "Several rules must still mean exactly one target group."
  }

  assert {
    condition = (
      length(output.rule_priorities) == 3
      && contains(output.rule_priorities, 10)
      && contains(output.rule_priorities, 11)
      && contains(output.rule_priorities, 12)
    )
    error_message = "The module must report every priority it occupies, so a root can check two services do not overlap."
  }

  # Exactly at the wildcard ceiling: three patterns of two wildcards each.
  assert {
    condition     = length(one(one(aws_lb_listener_rule.this["12"].condition).path_pattern).values) == 3
    error_message = "The session-scoped rule must carry its three anchored patterns."
  }
}

run "a_second_service_takes_a_different_priority_on_the_same_listener" {
  command = plan

  variables {
    name = "aex-dev-euw1-session"

    rules = [
      {
        priority      = 20
        path_patterns = ["/api/workspace/*"]
      },
    ]
  }

  assert {
    condition     = aws_lb_listener_rule.this["20"].priority == 20
    error_message = "A second service must be able to attach to the same listener at its own priority."
  }

  assert {
    condition     = aws_lb_target_group.this.name == "aex-dev-euw1-session-tg"
    error_message = "Each service gets its own target group, named after itself."
  }
}

run "the_rules_forward_only_the_api_prefix" {
  command = plan

  variables {
    rules = [
      {
        priority      = 10
        path_patterns = ["/api/sessions/*", "/api/workspace/*"]
      },
      {
        priority      = 11
        path_patterns = ["/api/streams/*"]
      },
    ]
  }

  assert {
    condition = alltrue(flatten([
      for key, rule in aws_lb_listener_rule.this : [
        for p in one(one(rule.condition).path_pattern).values :
        startswith(p, "/api/") && !startswith(p, "/internal")
      ]
    ]))
    error_message = "Every rule must forward only public paths under /api/, and never an /internal path."
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
    rules = [
      {
        priority      = 10
        path_patterns = ["/internal/*"]
      },
    ]
  }

  expect_failures = [var.rules]
}

run "rejects_forwarding_everything" {
  command = plan

  variables {
    rules = [
      {
        priority      = 10
        path_patterns = ["/*"]
      },
    ]
  }

  expect_failures = [var.rules]
}

run "rejects_a_pattern_outside_the_api_prefix_in_any_rule" {
  command = plan

  variables {
    rules = [
      {
        priority      = 10
        path_patterns = ["/api/sessions/*"]
      },
      {
        priority      = 11
        path_patterns = ["/healthz"]
      },
    ]
  }

  expect_failures = [var.rules]
}

run "rejects_an_empty_rule_list" {
  command = plan

  variables {
    rules = []
  }

  expect_failures = [var.rules]
}

run "rejects_a_rule_with_no_patterns" {
  command = plan

  variables {
    rules = [
      {
        priority      = 10
        path_patterns = []
      },
    ]
  }

  expect_failures = [var.rules]
}

# `Condition Values per Rule` is a hard, non-adjustable AWS quota of 5. Six
# patterns are legal for the service, but not in one rule.
run "rejects_a_rule_that_exceeds_five_condition_values" {
  command = plan

  variables {
    rules = [
      {
        priority = 10
        path_patterns = [
          "/api/example-a/*",
          "/api/example-b/*",
          "/api/example-c/*",
          "/api/example-d/*",
          "/api/example-e/*",
          "/api/example-f/*",
        ]
      },
    ]
  }

  expect_failures = [var.rules]
}

# The same six patterns split across two rules are accepted, which is the whole
# point: the quota is a per-rule bound, not a ceiling on the service.
run "accepts_those_same_six_patterns_split_across_two_rules" {
  command = plan

  variables {
    rules = [
      {
        priority      = 10
        path_patterns = ["/api/example-a/*", "/api/example-b/*", "/api/example-c/*", "/api/example-d/*", "/api/example-e/*"]
      },
      {
        priority      = 11
        path_patterns = ["/api/example-f/*"]
      },
    ]
  }

  assert {
    condition     = length(aws_lb_listener_rule.this) == 2
    error_message = "A surface too wide for one rule must be expressible as two."
  }
}

# `Condition Wildcards per Rule` is a hard, non-adjustable AWS quota of 6.
# Four two-wildcard patterns are eight wildcards in one rule.
run "rejects_a_rule_that_exceeds_six_wildcards" {
  command = plan

  variables {
    rules = [
      {
        priority = 10
        path_patterns = [
          "/api/examples/*/events/*",
          "/api/examples/*/logs/*",
          "/api/examples/*/metrics/*",
          "/api/examples/*/spans/*",
        ]
      },
    ]
  }

  expect_failures = [var.rules]
}

run "rejects_two_rules_sharing_a_priority" {
  command = plan

  variables {
    rules = [
      {
        priority      = 10
        path_patterns = ["/api/sessions/*"]
      },
      {
        priority      = 10
        path_patterns = ["/api/workspace/*"]
      },
    ]
  }

  expect_failures = [var.rules]
}

run "rejects_a_priority_outside_the_listener_range" {
  command = plan

  variables {
    rules = [
      {
        priority      = 0
        path_patterns = ["/api/streams/*"]
      },
    ]
  }

  expect_failures = [var.rules]
}

run "rejects_a_fractional_priority" {
  command = plan

  variables {
    rules = [
      {
        priority      = 10.5
        path_patterns = ["/api/streams/*"]
      },
    ]
  }

  expect_failures = [var.rules]
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
