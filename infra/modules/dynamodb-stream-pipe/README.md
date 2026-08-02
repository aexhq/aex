# `dynamodb-stream-pipe`

An EventBridge Pipe from one DynamoDB stream to one SQS queue, with its own
role.

The pipe carries a typed hint, not a raw database record. The consumer re-reads
the authority; the pipe only tells it where work may exist. That is why both the
filter pattern and target input template are mandatory: an unfiltered or
untransformed pipe either turns a table into a firehose or exposes the DynamoDB
Streams envelope as an accidental consumer contract.

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
| `input_template` | `string` | Per-record target input template. Mandatory, at most 8192 characters. |
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
- The input template reaching the target is the template supplied, verbatim.
- The pipe role grants `sqs:SendMessage` exactly once, scoped to the target
  queue, with no wildcard SQS action.
- The three scopable DynamoDB Streams calls name only the source stream;
  `ListStreams`, for which DynamoDB defines no resource type, is isolated on
  `Resource = "*"` and restricted to the source region.
- Only `pipes.amazonaws.com` may assume the role, and the trust is restricted to
  the source account and exact pipe ARN.
- An encrypted target grants only `kms:GenerateDataKey`, restricted to SQS and
  the target queue's encryption context.
- The target must be a queue ARN; any other target type is rejected.

## Not asserted here

Stream shard behaviour, ordering and iterator age are the
`aws.dynamodb.streams` seam.
