# `sqs-queue`

One SQS queue and its mandatory dead-letter queue, standard or FIFO, always
encrypted with a customer-managed key.

Both queues are created together. There is no configuration in which the module
produces a queue without a redrive target, because a message that cannot be
handled has to land somewhere an operator can look at it.

## Inputs

| Name | Type | Description |
| --- | --- | --- |
| `name` | `string` | Base queue name; the `.fifo` suffix is appended by the module. |
| `fifo` | `bool` | FIFO queue. Required when the name starts with `usage-rating`. |
| `content_based_dedup` | `bool` | FIFO deduplication from the body instead of an explicit id. |
| `visibility_timeout` | `number` | Visibility timeout in seconds, 1-43200. |
| `max_receive_count` | `number` | Deliveries before redrive to the dead-letter queue. |
| `message_retention_seconds` | `number` | Retention of the main queue. |
| `kms_key_arn` | `string` | Customer-managed key ARN. Mandatory. |
| `kms_data_key_reuse_period_seconds` | `number` | Data key reuse window. |
| `dlq` | `object` | `{ enabled, message_retention_seconds }`. `enabled` must be `true`. |
| `policy_statements` | `list(object)` | Resource policy statements, scoped to this queue by the module. |
| `tags` | `map(string)` | Tags applied to both queues. |

## Outputs

| Name | Description |
| --- | --- |
| `url` | URL of the main queue. |
| `arn` | ARN of the main queue. |
| `dlq_url` | URL of the dead-letter queue. |
| `dlq_arn` | ARN of the dead-letter queue. |
| `queue_name` | Physical name of the main queue including any `.fifo` suffix. |

## Policy asserted

- FIFO is required for any queue whose name starts with `usage-rating`; a
  standard configuration is rejected by validation.
- A dead-letter queue is mandatory. `dlq.enabled = false` is rejected, and the
  main queue always carries a redrive policy pointing at it.
- Server-side encryption with a customer-managed key is mandatory; the ARN is
  shape-checked and SQS-managed encryption is explicitly disabled.
- The queue policy contains no wildcard principal and no wildcard action, and
  every statement is scoped to this queue's ARN.
- FIFO is supported, including the `.fifo` suffix on both queues and per-message-group
  throughput.

## Not asserted here

Deduplication-window exactness, real redrive timing and dead-letter behaviour
under load belong to the `aws.sqs.fifo` seam.
