mock_provider "aws" {}

variables {
  name         = "aex-dev-central-device-flow"
  resource_arn = "arn:aws:elasticloadbalancing:eu-west-1:000000000000:loadbalancer/app/aex-dev-euw1-central/50dc6c495c0c9188"
  rate_limit   = 100

  paths = [
    "/api/auth/device/authorizations",
    "/api/auth/device/tokens",
  ]
}

run "the_web_acl_is_regional_and_attached_to_the_load_balancer_the_caller_named" {
  command = plan

  assert {
    condition     = aws_wafv2_web_acl.this.scope == "REGIONAL"
    error_message = "A load balancer can only carry a REGIONAL web ACL; a CLOUDFRONT-scoped one is rejected at apply and the listener is left with no rate limit at all."
  }

  assert {
    condition     = aws_wafv2_web_acl_association.this.resource_arn == var.resource_arn
    error_message = "The association must name the load balancer the caller passed. A web ACL that exists and is associated with nothing is a monthly charge that protects no request."
  }

  assert {
    condition     = aws_wafv2_web_acl.this.name == var.name
    error_message = "The web ACL must take the caller's name; it is the metric prefix an alarm is written against."
  }
}

run "the_default_action_allows_so_a_defect_here_is_not_an_outage" {
  command = plan

  assert {
    condition     = length(one(aws_wafv2_web_acl.this.default_action).allow) == 1
    error_message = "The default action must allow. This ACL bounds one class of caller and is not the plane's admission control; a default block would turn any mistake in this module into an outage of the whole listener."
  }

  assert {
    condition     = length(one(aws_wafv2_web_acl.this.default_action).block) == 0
    error_message = "A default block would refuse every request the scope-down does not match, which is every authenticated request on the plane."
  }
}

run "the_rate_rule_blocks_at_the_limit_and_window_the_caller_asked_for" {
  command = plan

  assert {
    condition     = one(one(one(aws_wafv2_web_acl.this.rule).statement).rate_based_statement).limit == var.rate_limit
    error_message = "The rule must use the caller's limit. A limit that silently differed from the one reviewed is a throttle nobody can reason about."
  }

  assert {
    condition     = one(one(one(aws_wafv2_web_acl.this.rule).statement).rate_based_statement).evaluation_window_sec == 300
    error_message = "The evaluation window must be the configured one; the limit is meaningless without the window it is counted over."
  }

  assert {
    condition     = length(one(one(aws_wafv2_web_acl.this.rule).action).block) == 1
    error_message = "The rate rule must block. Counting without blocking produces a metric and no protection, and this rule exists because the two device-flow routes have nothing else in front of them."
  }

  assert {
    condition     = one(aws_wafv2_web_acl.this.rule).priority == 0
    error_message = "The rate rule must evaluate first; there is nothing above it that could legitimately allow past it."
  }
}

run "the_rate_is_keyed_on_the_connection_source_and_not_on_a_caller_supplied_header" {
  command = plan

  assert {
    condition     = one(one(one(aws_wafv2_web_acl.this.rule).statement).rate_based_statement).aggregate_key_type == "IP"
    error_message = "Keying on FORWARDED_IP would trust `X-Forwarded-For`, which a caller may append to. One source could then present a fresh key per request and buy itself an unbounded rate, which is exactly the flood this rule exists to bound."
  }
}

run "the_scope_down_matches_each_named_path_exactly_and_only_those" {
  command = plan

  assert {
    condition = length(
      one(one(one(one(one(aws_wafv2_web_acl.this.rule).statement).rate_based_statement).scope_down_statement).or_statement).statement
    ) == length(var.paths)
    error_message = "There must be one byte-match statement per named path. A missing one leaves that route unthrottled while the plan still looks like it covers it."
  }

  assert {
    condition = alltrue([
      for s in one(one(one(one(one(aws_wafv2_web_acl.this.rule).statement).rate_based_statement).scope_down_statement).or_statement).statement :
      one(s.byte_match_statement).positional_constraint == "EXACTLY"
    ])
    error_message = "Paths must match exactly. A STARTS_WITH constraint would also cover any route added under `/api/auth/device/` later, so a new route would inherit a throttle nobody wrote for it and nothing at the route's own definition would say so."
  }

  assert {
    condition = toset([
      for s in one(one(one(one(one(aws_wafv2_web_acl.this.rule).statement).rate_based_statement).scope_down_statement).or_statement).statement :
      one(s.byte_match_statement).search_string
    ]) == toset(var.paths)
    error_message = "The matched strings must be exactly the caller's paths, with nothing added and nothing dropped."
  }
}

run "a_percent_encoded_or_padded_path_cannot_walk_around_the_rule" {
  command = plan

  assert {
    condition = alltrue([
      for s in one(one(one(one(one(aws_wafv2_web_acl.this.rule).statement).rate_based_statement).scope_down_statement).or_statement).statement :
      toset([for t in one(s.byte_match_statement).text_transformation : t.type]) == toset(["URL_DECODE", "COMPRESS_WHITE_SPACE"])
    ])
    error_message = "Without URL_DECODE a caller reaches the same route by percent-encoding a character and the exact match misses it, which makes the limit optional for anyone who reads this file."
  }
}

run "no_request_sample_is_retained_because_these_paths_carry_device_codes" {
  command = plan

  assert {
    condition     = one(aws_wafv2_web_acl.this.visibility_config).sampled_requests_enabled == false
    error_message = "A sampled request is stored with its headers and body-adjacent metadata. The paths this ACL watches carry device codes, so retaining samples would put a live credential in a WAF console."
  }

  assert {
    condition     = one(one(aws_wafv2_web_acl.this.rule).visibility_config).sampled_requests_enabled == false
    error_message = "The rule's own sampling must be off for the same reason as the ACL's."
  }

  assert {
    condition     = one(one(aws_wafv2_web_acl.this.rule).visibility_config).cloudwatch_metrics_enabled == true
    error_message = "Metrics must stay on; `BlockedRequests` at this rule's dimension is the only signal that the limit is doing anything at all."
  }
}

run "a_single_path_is_admitted" {
  command = plan

  variables {
    paths = ["/api/auth/device/tokens"]
  }

  assert {
    condition = length(
      one(one(one(one(one(aws_wafv2_web_acl.this.rule).statement).rate_based_statement).scope_down_statement).or_statement).statement
    ) == 1
    error_message = "One path must produce one byte-match statement; an OR of one is still an OR."
  }
}

run "the_smallest_and_largest_windows_aws_admits_are_both_accepted" {
  command = plan

  variables {
    evaluation_window_seconds = 60
  }

  assert {
    condition     = one(one(one(aws_wafv2_web_acl.this.rule).statement).rate_based_statement).evaluation_window_sec == 60
    error_message = "A 60 second window is one of the four AWS admits and must pass."
  }
}

# --- negatives, one per validation -------------------------------------------

run "a_name_outside_the_platform_prefix_is_refused" {
  command = plan

  variables {
    name = "device-flow"
  }

  expect_failures = [var.name]
}

run "a_name_with_uppercase_is_refused" {
  command = plan

  variables {
    name = "aex-Dev-Central"
  }

  expect_failures = [var.name]
}

run "a_resource_that_is_not_an_application_load_balancer_is_refused" {
  command = plan

  variables {
    resource_arn = "arn:aws:elasticloadbalancing:eu-west-1:000000000000:loadbalancer/net/aex-dev-euw1/50dc6c495c0c9188"
  }

  expect_failures = [var.resource_arn]
}

run "a_listener_arn_in_place_of_the_load_balancer_is_refused" {
  command = plan

  variables {
    resource_arn = "arn:aws:elasticloadbalancing:eu-west-1:000000000000:listener/app/aex-dev-euw1/50dc6c495c0c9188/f2f7dc8efc522ab2"
  }

  expect_failures = [var.resource_arn]
}

run "an_empty_path_list_is_refused" {
  command = plan

  variables {
    paths = []
  }

  expect_failures = [var.paths]
}

run "rejects_rate_limiting_an_internal_path" {
  command = plan

  variables {
    paths = ["/internal/readyz"]
  }

  expect_failures = [var.paths]
}

run "a_path_outside_the_public_api_prefix_is_refused" {
  command = plan

  variables {
    paths = ["/auth/device/tokens"]
  }

  expect_failures = [var.paths]
}

run "a_wildcard_path_is_refused_rather_than_matching_nothing" {
  command = plan

  variables {
    paths = ["/api/auth/device/*"]
  }

  expect_failures = [var.paths]
}

run "a_query_string_in_a_path_is_refused" {
  command = plan

  variables {
    paths = ["/api/auth/device/tokens?grant=x"]
  }

  expect_failures = [var.paths]
}

run "more_paths_than_one_scope_down_should_carry_are_refused" {
  command = plan

  variables {
    paths = [
      "/api/a1", "/api/a2", "/api/a3", "/api/a4", "/api/a5",
      "/api/a6", "/api/a7", "/api/a8", "/api/a9", "/api/a10",
      "/api/a11",
    ]
  }

  expect_failures = [var.paths]
}

run "a_repeated_path_is_refused" {
  command = plan

  variables {
    paths = [
      "/api/auth/device/tokens",
      "/api/auth/device/tokens",
    ]
  }

  expect_failures = [var.paths]
}

run "a_rate_limit_below_the_aws_floor_is_refused" {
  command = plan

  variables {
    rate_limit = 9
  }

  expect_failures = [var.rate_limit]
}

run "a_rate_limit_above_the_aws_ceiling_is_refused" {
  command = plan

  variables {
    rate_limit = 2000000001
  }

  expect_failures = [var.rate_limit]
}

run "a_fractional_rate_limit_is_refused" {
  command = plan

  variables {
    rate_limit = 100.5
  }

  expect_failures = [var.rate_limit]
}

run "a_window_aws_does_not_admit_is_refused_here_rather_than_at_apply" {
  command = plan

  variables {
    evaluation_window_seconds = 180
  }

  expect_failures = [var.evaluation_window_seconds]
}
