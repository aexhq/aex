output "dns_name" {
  value       = aws_lb.this.dns_name
  description = "DNS name of the load balancer."
}

output "listener_arn" {
  value       = aws_lb_listener.https.arn
  description = "ARN of the HTTPS listener."
}

output "target_group_arn" {
  value       = aws_lb_target_group.this.arn
  description = "ARN of the target group services register with."
}

output "zone_id" {
  value       = aws_lb.this.zone_id
  description = "Hosted zone id of the load balancer, for an alias record."
}

output "deregistration_delay" {
  value       = var.deregistration_delay
  description = "Drain window configured on the target group. A service behind this load balancer must not stop draining sooner."
}
