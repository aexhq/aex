output "target_group_arn" {
  value       = aws_lb_target_group.this.arn
  description = "ARN of the target group this service registers with."
}

output "deregistration_delay" {
  value       = var.deregistration_delay
  description = "Drain window configured on the target group. The service behind it must not stop draining sooner, so the load balancer and the task cannot disagree."
}

output "rule_priorities" {
  value       = [for rule in var.rules : rule.priority]
  description = "Priorities this service occupies on the listener, in declaration order. A root composing several services can assert their sets do not overlap; this module can only see its own."
}
