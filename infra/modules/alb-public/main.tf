locals {
  security_group_ids = concat([aws_security_group.this.id], var.additional_security_group_ids)
}

# The edge's ingress is the edge's to own. While no module created this group
# and no environment root was allowed to declare one, it could not exist
# anywhere, and every root that wired `security_group_ids` was naming a group
# nothing in the repository builds.
#
# This group has no egress rule of its own on purpose. Terraform revokes the
# allow-all egress AWS attaches to a new group, and the only thing this load
# balancer ever needs to reach is a service target: `ecs-service` opens exactly
# the container port of the service it stands up, against exactly that service's
# task group. The load balancer cannot name those groups itself without
# depending on the services that already depend on it.
resource "aws_security_group" "this" {
  name        = "${var.name}-alb"
  description = "Public HTTP and HTTPS ingress to the ${var.name} load balancer"
  vpc_id      = var.vpc_id

  tags = merge(var.tags, { Name = "${var.name}-alb" })
}

resource "aws_vpc_security_group_ingress_rule" "https" {
  security_group_id = aws_security_group.this.id
  description       = "HTTPS from the public internet"
  ip_protocol       = "tcp"
  from_port         = 443
  to_port           = 443
  cidr_ipv4         = "0.0.0.0/0"
}

# Port 80 is admitted so the redirect below can answer. Closing it would not
# make the edge smaller, it would make a plain `http://` request time out
# instead of returning the 301 this module already declares.
resource "aws_vpc_security_group_ingress_rule" "http_redirect" {
  security_group_id = aws_security_group.this.id
  description       = "HTTP from the public internet, answered only by the permanent redirect to HTTPS"
  ip_protocol       = "tcp"
  from_port         = 80
  to_port           = 80
  cidr_ipv4         = "0.0.0.0/0"
}

resource "aws_lb" "this" {
  name                       = var.name
  load_balancer_type         = "application"
  internal                   = false
  subnets                    = var.subnet_ids
  security_groups            = local.security_group_ids
  idle_timeout               = var.idle_timeout
  drop_invalid_header_fields = true
  enable_http2               = true
  tags                       = var.tags

  access_logs {
    enabled = true
    bucket  = var.access_logs_bucket
    prefix  = var.access_logs_prefix
  }
}

# Anything no `alb-service-target` rule matches gets this 404. `/internal/*` is
# therefore unreachable from the public listener even though the service target
# groups health-check it.
resource "aws_lb_listener" "https" {
  load_balancer_arn = aws_lb.this.arn
  port              = 443
  protocol          = "HTTPS"
  ssl_policy        = var.ssl_policy
  certificate_arn   = var.certificate_arn
  tags              = var.tags

  default_action {
    type = "fixed-response"

    fixed_response {
      content_type = "application/json"
      status_code  = "404"
      message_body = "{\"error\":{\"code\":\"not_found\"}}"
    }
  }
}

resource "aws_lb_listener" "http_redirect" {
  load_balancer_arn = aws_lb.this.arn
  port              = 80
  protocol          = "HTTP"
  tags              = var.tags

  default_action {
    type = "redirect"

    redirect {
      port        = "443"
      protocol    = "HTTPS"
      status_code = "HTTP_301"
    }
  }
}
