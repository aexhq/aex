output "url" {
  value       = aws_sqs_queue.this.url
  description = "URL of the main queue."
}

output "arn" {
  value       = aws_sqs_queue.this.arn
  description = "ARN of the main queue."
}

output "dlq_url" {
  value       = aws_sqs_queue.dlq.url
  description = "URL of the dead-letter queue."
}

output "dlq_arn" {
  value       = aws_sqs_queue.dlq.arn
  description = "ARN of the dead-letter queue."
}

output "queue_name" {
  value       = local.queue_name
  description = "Physical name of the main queue, including the `.fifo` suffix when the queue is FIFO."
}
