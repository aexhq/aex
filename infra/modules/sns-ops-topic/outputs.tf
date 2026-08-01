output "topic_arn" {
  value       = aws_sns_topic.this.arn
  description = "ARN of the operational topic."
}

output "subscription_arns" {
  value       = { for k, s in aws_sns_topic_subscription.this : k => s.arn }
  description = "Subscription key to subscription ARN."
}
