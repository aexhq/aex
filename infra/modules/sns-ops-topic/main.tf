locals {
  topic_arn = "arn:${var.partition}:sns:${var.region}:${var.account_id}:${var.name}"

  topic_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      for i, p in var.publish_principals : {
        Sid       = "AllowPublish${i}"
        Effect    = "Allow"
        Principal = { (p.type) = p.identifier }
        Action    = ["sns:Publish"]
        Resource  = local.topic_arn
      }
    ]
  })
}

resource "aws_sns_topic" "this" {
  name              = var.name
  kms_master_key_id = var.kms_key_arn
  tags              = var.tags
}

resource "aws_sns_topic_policy" "this" {
  arn    = aws_sns_topic.this.arn
  policy = local.topic_policy
}

resource "aws_sns_topic_subscription" "this" {
  for_each = { for s in var.subscriptions : "${s.protocol}:${s.endpoint}" => s }

  topic_arn = aws_sns_topic.this.arn
  protocol  = each.value.protocol
  endpoint  = each.value.endpoint
}
