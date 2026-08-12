resource "terraform_data" "queue" {
  input = "arn:aws:sqs:eu-west-1:000000000000:aex-dev-control-wake-dlq"
}

output "queue_arn" {
  value       = terraform_data.queue.output
  description = "A valid queue ARN whose value remains unknown during a plan-only test run."
}
