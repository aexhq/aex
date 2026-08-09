output "dns_name" {
  value       = aws_lb.this.dns_name
  description = "DNS name of the load balancer."
}

output "arn" {
  value       = aws_lb.this.arn
  description = "ARN of the load balancer itself. A `waf-rate-limit` web ACL associates with this, not with the listener: `aws_wafv2_web_acl_association` takes the load balancer and AWS refuses a listener ARN."
}

output "listener_arn" {
  value       = aws_lb_listener.https.arn
  description = "ARN of the HTTPS listener."
}

output "zone_id" {
  value       = aws_lb.this.zone_id
  description = "Hosted zone id of the load balancer, for an alias record."
}

output "security_group_id" {
  value       = aws_security_group.this.id
  description = "The load balancer's own security group. It admits TCP/443 and TCP/80 from the public internet and nothing else. A service behind this load balancer names it to admit exactly this edge on its container port; that service also opens the matching egress here, because only it knows which port to open."
}

output "deregistration_delay" {
  value       = var.deregistration_delay
  description = "Drain window this load balancer publishes for every service attached to it. A root passes it to each `alb-service-target` and to the service behind it, so the two cannot disagree."
}
