# `dynamodb-stream-pipe`

An EventBridge Pipe from one DynamoDB stream to one SQS queue, with its own
role.

The pipe carries a hint, not a payload of record. The consumer re-reads the
table; the pipe only tells it there is something to look at. That is why the
filter pattern is mandatory: an unfiltered pipe forwards every mutation and
turns a table into a firehose.

The role this module creates is deliberately the only writer it grants on the
target queue. Nothing here hands `sqs:SendMessage` to a service role as well,
because a second writer would make the consumer's ordering assumption
unprovable.

## Inputs

| Name | Type | Description |
| --- | --- | --- |
| `name` | `string` | Pipe name, `aex-<...>`. |
| `source_stream_arn` | `string` | DynamoDB stream ARN. |
| `target_queue_arn` | `string` | SQS queue ARN. |
| `filter_pattern` | `string` | EventBridge filter pattern as JSON. Mandatory. |
| `batch_size` | `number` | 1-10000. |
| `starting_position` | `string` | `TRIM_HORIZON` or `LATEST`. |
| `maximum_batching_window` | `number` | 0-300 seconds. |
| `stream_kms_key_arn` | `string` | Optional key for stream decrypt. |
| `target_kms_key_arn` | `string` | Optional key for queue encrypt. |
| `tags` | `map(string)` | Tags. |

## Outputs

| Name | Description |
| --- | --- |
| `pipe_arn` | ARN of the pipe. |
| `pipe_role_arn` | ARN of the pipe role. |

## Policy asserted

- The filter pattern is non-empty, parses as JSON and decodes to a non-empty
  object. An empty string, an unparseable string and `{}` are all rejected.
- The pattern reaching the pipe is the pattern that was supplied, verbatim.
- The pipe role grants `sqs:SendMessage` exactly once, scoped to the target
  queue, with no wildcard SQS action.
- Only `pipes.amazonaws.com` may assume the role, so this module grants queue
  write access to no service role of its own.
- The target must be a queue ARN; any other target type is rejected.

## Not asserted here

Stream shard behaviour, ordering and iterator age are the
`aws.dynamodb.streams` seam.
