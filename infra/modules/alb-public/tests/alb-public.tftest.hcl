mock_provider "aws" {}

variables {
  name               = "aex-dev-euw1-public"
  vpc_id             = "vpc-0123456789abcdef0"
  subnet_ids         = ["subnet-0123456789abcdef0", "subnet-0123456789abcdef1"]
  security_group_ids = ["sg-0123456789abcdef0"]
  certificate_arn    = "arn:aws:acm:eu-west-1:000000000000:certificate/00000000-0000-4000-8000-000000000000"
  target_port        = 8080
  access_logs_bucket = "aex-dev-alb-logs-0a1b2c3d"
}

run "the_idle_timeout_survives_a_long_stream" {
  command = plan

  assert {
    condition     = aws_lb.this.idle_timeout >= 1200
    error_message = "The idle timeout must be at least 1200 seconds."
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
}

run "exactly_one_listener_rule_forwards_only_the_api_prefix" {
  command = plan

  assert {
    condition     = aws_lb_listener_rule.api.priority == 1
    error_message = "The single listener rule must be the first rule."
  }

  assert {
    condition = alltrue([
      for p in one(one(aws_lb_listener_rule.api.condition).path_pattern).values :
      startswith(p, "/api/")
    ])
    error_message = "The listener rule must forward only paths under /api/."
  }

  assert {
    condition = alltrue([
      for p in one(one(aws_lb_listener_rule.api.condition).path_pattern).values :
      !startswith(p, "/internal")
    ])
    error_message = "The listener rule must never forward /internal paths."
  }

  assert {
    condition     = one(aws_lb_listener_rule.api.action).type == "forward"
    error_message = "The single rule must forward to the target group."
  }
}

run "internal_paths_are_unreachable_from_the_public_listener" {
  command = plan

  assert {
    condition     = one(aws_lb_listener.https.default_action).type == "fixed-response"
    error_message = "Anything the single rule does not match must get a fixed response, not the target group."
  }

  assert {
    condition     = one(one(aws_lb_listener.https.default_action).fixed_response).status_code == "404"
    error_message = "The default action must answer 404, so /internal/* is unreachable from the public listener."
  }
}

run "http_redirects_to_https" {
  command = plan

  assert {
    condition     = one(aws_lb_listener.http_redirect.default_action).type == "redirect"
    error_message = "The HTTP listener must redirect."
  }

  assert {
    condition     = one(one(aws_lb_listener.http_redirect.default_action).redirect).protocol == "HTTPS"
    error_message = "The HTTP listener must redirect to HTTPS."
  }

  assert {
    condition     = one(one(aws_lb_listener.http_redirect.default_action).redirect).status_code == "HTTP_301"
    error_message = "The redirect must be permanent."
  }
}

run "access_logs_are_on" {
  command = plan

  assert {
    condition     = one(aws_lb.this.access_logs).enabled == true
    error_message = "Access logging must be enabled."
  }

  assert {
    condition     = one(aws_lb.this.access_logs).bucket == var.access_logs_bucket
    error_message = "Access logs must go to the configured bucket."
  }
}

run "rejects_an_idle_timeout_below_twenty_minutes" {
  command = plan

  variables {
    idle_timeout = 60
  }

  expect_failures = [var.idle_timeout]
}

run "rejects_forwarding_an_internal_path" {
  command = plan

  variables {
    forward_path_patterns = ["/internal/*"]
  }

  expect_failures = [var.forward_path_patterns]
}

run "rejects_forwarding_everything" {
  command = plan

  variables {
    forward_path_patterns = ["/*"]
  }

  expect_failures = [var.forward_path_patterns]
}

run "rejects_a_public_health_check_path" {
  command = plan

  variables {
    health_check_path = "/healthz"
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

run "rejects_an_empty_access_log_bucket" {
  command = plan

  variables {
    access_logs_bucket = ""
  }

  expect_failures = [var.access_logs_bucket]
}
