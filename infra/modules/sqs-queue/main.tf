locals {
  suffix     = var.fifo ? ".fifo" : ""
  queue_name = "${var.name}${local.suffix}"
  dlq_name   = "${var.name}-dlq${local.suffix}"
}

resource "aws_sqs_queue" "dlq" {
  name                              = local.dlq_name
  fifo_queue                        = var.fifo
  content_based_deduplication       = var.fifo ? var.content_based_dedup : null
  message_retention_seconds         = var.dlq.message_retention_seconds
  visibility_timeout_seconds        = var.visibility_timeout
  kms_master_key_id                 = var.kms_key_arn
  kms_data_key_reuse_period_seconds = var.kms_data_key_reuse_period_seconds
  tags                              = var.tags
}

resource "aws_sqs_queue" "this" {
  name                              = local.queue_name
  fifo_queue                        = var.fifo
  content_based_deduplication       = var.fifo ? var.content_based_dedup : null
  deduplication_scope               = var.fifo ? "messageGroup" : null
  fifo_throughput_limit             = var.fifo ? "perMessageGroupId" : null
  message_retention_seconds         = var.message_retention_seconds
  visibility_timeout_seconds        = var.visibility_timeout
  kms_master_key_id                 = var.kms_key_arn
  kms_data_key_reuse_period_seconds = var.kms_data_key_reuse_period_seconds
  tags                              = var.tags

  redrive_policy = jsonencode({
    deadLetterTargetArn = aws_sqs_queue.dlq.arn
    maxReceiveCount     = var.max_receive_count
  })
}

resource "aws_sqs_queue_policy" "this" {
  count = length(var.policy_statements) > 0 ? 1 : 0

  queue_url = aws_sqs_queue.this.url

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      for s in var.policy_statements : {
        Sid       = s.sid
        Effect    = s.effect
        Principal = { (s.principal_type) = s.principals }
        Action    = s.actions
        Resource  = aws_sqs_queue.this.arn
      }
    ]
  })
}
