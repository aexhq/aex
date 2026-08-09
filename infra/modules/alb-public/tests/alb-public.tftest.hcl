mock_provider "aws" {}

# The group this module creates has no id until it exists, and the load
# balancer's attachment is asserted below, so the plan needs one.
override_resource {
  target          = aws_security_group.this
  override_during = plan
  values = {
    id = "sg-0123456789abcdef0"
  }
}

variables {
  name               = "aex-dev-euw1-public"
  vpc_id             = "vpc-0123456789abcdef0"
  subnet_ids         = ["subnet-0123456789abcdef0", "subnet-0123456789abcdef1"]
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

# --- the edge owns its own ingress ------------------------------------------

run "the_load_balancer_creates_the_group_it_runs_with" {
  command = plan

  assert {
    condition     = aws_security_group.this.vpc_id == var.vpc_id
    error_message = "The load balancer's security group must be created by this module, in the VPC its public subnets belong to. No module created these groups and no environment root may declare one, so a group nobody creates is a load balancer that cannot be applied at all."
  }

  assert {
    condition     = aws_security_group.this.name == "${var.name}-alb"
    error_message = "The group name must be derived from the load balancer's own name, never supplied, so two load balancers cannot be given the same group name by a caller."
  }

  assert {
    condition     = contains(aws_lb.this.security_groups, aws_security_group.this.id)
    error_message = "The load balancer must be attached to the group this module creates."
  }

  assert {
    condition     = length(aws_lb.this.security_groups) == 1
    error_message = "With no additional groups supplied, the module's own group must be the only one attached."
  }
}

run "the_public_edge_admits_tls_from_anywhere_and_nothing_else" {
  command = plan

  assert {
    condition = (
      aws_vpc_security_group_ingress_rule.https.ip_protocol == "tcp"
      && aws_vpc_security_group_ingress_rule.https.from_port == 443
      && aws_vpc_security_group_ingress_rule.https.to_port == 443
      && aws_vpc_security_group_ingress_rule.https.cidr_ipv4 == "0.0.0.0/0"
    )
    error_message = "A public load balancer admits TCP/443 from the internet. This is the one place in the network where an open CIDR is the intent rather than an oversight."
  }

  assert {
    condition     = aws_vpc_security_group_ingress_rule.https.security_group_id == aws_security_group.this.id
    error_message = "The ingress rule must land on the group this module creates."
  }
}

# The HTTP listener already redirects. Refusing the port would not shrink the
# edge, it would replace a 301 with a timeout.
run "the_redirect_listener_is_reachable" {
  command = plan

  assert {
    condition = (
      aws_vpc_security_group_ingress_rule.http_redirect.from_port == 80
      && aws_vpc_security_group_ingress_rule.http_redirect.to_port == 80
      && aws_vpc_security_group_ingress_rule.http_redirect.cidr_ipv4 == "0.0.0.0/0"
    )
    error_message = "Port 80 must be admitted, or the permanent redirect this module declares can never answer and a plain http:// request times out instead."
  }
}

run "additional_groups_are_added_to_the_module_group_not_substituted_for_it" {
  command = plan

  variables {
    additional_security_group_ids = ["sg-0123456789abcdef1"]
  }

  assert {
    condition     = contains(aws_lb.this.security_groups, aws_security_group.this.id)
    error_message = "The module's own group must stay attached whatever else a caller supplies; a caller may add to the public edge's ingress but may not replace it."
  }

  assert {
    condition     = length(aws_lb.this.security_groups) == 2
    error_message = "An additional group must be attached alongside the module's own, not instead of it."
  }
}

run "rejects_a_vpc_id_that_is_not_a_vpc" {
  command = plan

  variables {
    vpc_id = "subnet-0123456789abcdef0"
  }

  expect_failures = [var.vpc_id]
}

run "rejects_an_additional_group_that_is_not_a_security_group" {
  command = plan

  variables {
    additional_security_group_ids = ["aex-dev-euw1-public-alb"]
  }

  expect_failures = [var.additional_security_group_ids]
}

run "rejects_a_name_outside_the_aex_prefix" {
  command = plan

  variables {
    name = "public-lb"
  }

  expect_failures = [var.name]
}

run "rejects_a_single_subnet" {
  command = plan

  variables {
    subnet_ids = ["subnet-0123456789abcdef0"]
  }

  expect_failures = [var.subnet_ids]
}

run "rejects_a_certificate_that_is_not_an_acm_certificate" {
  command = plan

  variables {
    certificate_arn = "arn:aws:iam::000000000000:server-certificate/aex"
  }

  expect_failures = [var.certificate_arn]
}

run "rejects_a_tls_policy_below_one_three" {
  command = plan

  variables {
    ssl_policy = "ELBSecurityPolicy-TLS-1-2-2017-01"
  }

  expect_failures = [var.ssl_policy]
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
