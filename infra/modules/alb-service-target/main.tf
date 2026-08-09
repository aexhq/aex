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

# Exactly one rule, at the caller's explicit priority, forwarding exactly the
# configured public paths. The invariant `alb-public` used to hold by having a
# single rule now lives here and is held per service: one instance of this
# module is one target group and one rule, and the priority is required so two
# instances cannot silently claim the same slot.
resource "aws_lb_listener_rule" "this" {
  listener_arn = var.listener_arn
  priority     = var.priority
  tags         = var.tags

  condition {
    path_pattern {
      values = var.path_patterns
    }
  }

  action {
    type             = "forward"
    target_group_arn = aws_lb_target_group.this.arn
  }
}
