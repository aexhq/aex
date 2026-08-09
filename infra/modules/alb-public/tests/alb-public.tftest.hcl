mock_provider "aws" {}

variables {
  name               = "aex-dev-euw1-public"
  subnet_ids         = ["subnet-0123456789abcdef0", "subnet-0123456789abcdef1"]
  security_group_ids = ["sg-0123456789abcdef0"]
  certificate_arn    = "arn:aws:acm:eu-west-1:000000000000:certificate/00000000-0000-4000-8000-000000000000"
  access_logs_bucket = "aex-dev-alb-logs-0a1b2c3d"
}

run "the_idle_timeout_survives_a_long_stream" {
  command = plan

  assert {
    condition     = aws_lb.this.idle_timeout >= 1200
    error_message = "The idle timeout must be at least 1200 seconds."
  }
}

run "the_drain_window_is_published_for_every_service_behind_it" {
  command = plan

  assert {
    condition     = output.deregistration_delay >= 30
    error_message = "The load balancer must publish a drain window of at least 30 seconds for the service targets attached to it."
  }
}

# The target group and the listener rule moved to `alb-service-target`, so this
# module no longer routes anything. What it still owns is the listener whose
# default action makes every unmatched path - `/internal/*` above all - a 404.
run "this_module_routes_nothing_itself" {
  command = plan

  assert {
    condition     = length(aws_lb_listener.https.default_action) == 1
    error_message = "The HTTPS listener must carry exactly one default action and no forwarding of its own."
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
