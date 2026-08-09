resource "aws_lb_target_group" "this" {
  name                 = "${var.name}-tg"
  port                 = var.target_port
  protocol             = "HTTP"
  target_type          = "ip"
  vpc_id               = var.vpc_id
  deregistration_delay = var.deregistration_delay
  tags                 = var.tags

  health_check {
    path                = var.health_check_path
    protocol            = "HTTP"
    interval            = var.health_check.interval
    timeout             = var.health_check.timeout
    healthy_threshold   = var.health_check.healthy_threshold
    unhealthy_threshold = var.health_check.unhealthy_threshold
    matcher             = var.health_check.matcher
  }
}

# One rule per element, every one of them forwarding to the single target group
# above. The invariant `alb-public` used to hold by having a single rule now
# lives here and is held per service: one instance of this module is one
# service's whole attachment, and every priority in it is required and explicit
# so no rule can silently claim a slot another service was using.
#
# Keyed by priority, which the variable validates as unique, so a rule's address
# in state does not shift when the list is reordered.
resource "aws_lb_listener_rule" "this" {
  for_each = { for rule in var.rules : tostring(rule.priority) => rule }

  listener_arn = var.listener_arn
  priority     = each.value.priority
  tags         = var.tags

  condition {
    path_pattern {
      values = each.value.path_patterns
    }
  }

  action {
    type             = "forward"
    target_group_arn = aws_lb_target_group.this.arn
  }
}
