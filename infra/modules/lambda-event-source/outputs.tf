output "uuid" {
  value       = aws_lambda_event_source_mapping.this.uuid
  description = "Identifier of the event source mapping."
}
