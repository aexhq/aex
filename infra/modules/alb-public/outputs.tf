output "dns_name" {
  value       = aws_lb.this.dns_name
  description = "DNS name of the load balancer."
}

output "listener_arn" {
  value       = aws_lb_listener.https.arn
  description = "ARN of the HTTPS listener."
}

output "zone_id" {
  value       = aws_lb.this.zone_id
  description = "Hosted zone id of the load balancer, for an alias record."
}

output "deregistration_delay" {
  value       = var.deregistration_delay
  description = "Drain window this load balancer publishes for every service attached to it. A root passes it to each `alb-service-target` and to the service behind it, so the two cannot disagree."
}
