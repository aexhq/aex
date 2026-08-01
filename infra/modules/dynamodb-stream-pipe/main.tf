locals {
  role_name = "${var.name}-pipe"

  # The pipe role is the only writer this module creates. It can send to the one
  # target queue and read the one source stream, and it grants nothing to any
  # service role: a second writer on the same queue would make the ordering
  # guarantee the consumer relies on unprovable.
  pipe_policy = jsonencode({
    Version = "2012-10-17"
    Statement = concat(
      [
        {
          Sid    = "ReadSourceStream"
          Effect = "Allow"
          Action = [
            "dynamodb:DescribeStream",
            "dynamodb:GetRecords",
            "dynamodb:GetShardIterator",
            "dynamodb:ListStreams",
          ]
          Resource = var.source_stream_arn
        },
        {
          Sid      = "WriteTargetQueue"
          Effect   = "Allow"
          Action   = ["sqs:SendMessage"]
          Resource = var.target_queue_arn
        },
      ],
      var.stream_kms_key_arn == null ? [] : [
        {
          Sid      = "DecryptSourceStream"
          Effect   = "Allow"
          Action   = ["kms:Decrypt"]
          Resource = var.stream_kms_key_arn
        },
      ],
      var.target_kms_key_arn == null ? [] : [
        {
          Sid      = "EncryptTargetQueue"
          Effect   = "Allow"
          Action   = ["kms:Encrypt", "kms:GenerateDataKey"]
          Resource = var.target_kms_key_arn
        },
      ],
    )
  })
}

resource "aws_iam_role" "pipe" {
  name = local.role_name

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid       = "AssumeByPipes"
        Effect    = "Allow"
        Principal = { Service = ["pipes.amazonaws.com"] }
        Action    = "sts:AssumeRole"
      },
    ]
  })

  tags = var.tags
}

resource "aws_iam_role_policy" "pipe" {
  name   = "${local.role_name}-inline"
  role   = aws_iam_role.pipe.id
  policy = local.pipe_policy
}

resource "aws_pipes_pipe" "this" {
  name     = var.name
  role_arn = aws_iam_role.pipe.arn
  source   = var.source_stream_arn
  target   = var.target_queue_arn
  tags     = var.tags

  source_parameters {
    dynamodb_stream_parameters {
      starting_position                  = var.starting_position
      batch_size                         = var.batch_size
      maximum_batching_window_in_seconds = var.maximum_batching_window
    }

    filter_criteria {
      filter {
        pattern = var.filter_pattern
      }
    }
  }
}
