output "web_acl_arn" {
  value       = aws_wafv2_web_acl.this.arn
  description = "The web ACL ARN, for an alarm or a second association."
}

output "web_acl_id" {
  value       = aws_wafv2_web_acl.this.id
  description = "The web ACL id."
}

output "metric_name" {
  value       = aws_wafv2_web_acl.this.visibility_config[0].metric_name
  description = "The CloudWatch metric name the ACL publishes under, so an alarm names the same string the ACL does rather than re-deriving it."
}

output "rule_metric_name" {
  value       = "${var.name}-rate"
  description = "The rate rule's own metric name. `BlockedRequests` at this dimension is the only signal that the limit is doing anything."
}

output "rate_limit" {
  value       = var.rate_limit
  description = "The configured per-source-IP limit, echoed so a caller can assert on what it asked for."
}

output "protected_paths" {
  value       = var.paths
  description = "The exact paths the scope-down statement matches."
}
