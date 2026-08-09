output "target_group_arn" {
  value       = aws_lb_target_group.this.arn
  description = "ARN of the target group this service registers with."
}

output "deregistration_delay" {
  value       = var.deregistration_delay
  description = "Drain window configured on the target group. The service behind it must not stop draining sooner, so the load balancer and the task cannot disagree."
}
