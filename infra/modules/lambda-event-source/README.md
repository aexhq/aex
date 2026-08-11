# `lambda-event-source`

One event source mapping from an SQS queue or a DynamoDB stream to a Lambda
alias.

Two rules make this module worth having as a module rather than a resource. The
target is always an alias, and the function always reports per-record failures.
Both are easy to forget and both fail quietly: an unqualified target silently
changes behaviour on the next publish, and a missing
`ReportBatchItemFailures` turns one poison record into a redriven batch.

## Inputs

| Name | Type | Description |
| --- | --- | --- |
| `function_alias_arn` | `string` | Qualified alias ARN; never `$LATEST`. |
| `source_arn` | `string` | SQS queue ARN or DynamoDB stream ARN. |
| `batch_size` | `number` | 1-10000. |
| `max_batching_window` | `number` | 0-300 seconds. |
| `scaling_config` | `object` | `{ maximum_concurrency }`; SQS only. |
| `partial_batch_response` | `bool` | Must be `true`. |
| `starting_position` | `string` | Required for a stream, forbidden for a queue. |
| `enabled` | `bool` | Whether the mapping is enabled. |
| `tags` | `map(string)` | Tags. |

## Outputs

| Name | Description |
| --- | --- |
| `uuid` | Identifier of the event source mapping. |

## Policy asserted

- `function_response_types = ["ReportBatchItemFailures"]` is always requested;
  `partial_batch_response = false` is rejected.
- The target is a qualified alias ARN. An unqualified function ARN and a
  `$LATEST` qualifier are both rejected.
- A DynamoDB stream source must declare a starting position; a queue source must
  not.
- A scaling configuration is accepted only on a queue source.

## Not asserted here

Redrive timing, visibility interaction and partial-batch behaviour under load
belong to the `aws.sqs.fifo` and `aws.dynamodb.streams` seams.
