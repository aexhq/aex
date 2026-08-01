output "arn" {
  value       = aws_ecs_cluster.this.arn
  description = "Cluster ARN, the value a service or a one-shot task names."
}

output "name" {
  value       = aws_ecs_cluster.this.name
  description = "Physical cluster name, for the log-driver and task ARNs that need it as a string."
}
