# `region-foundation`

The stateful half of one region: the network, the authority keys, the DynamoDB
tables and the content bucket.

It is separate from `region-application` because these resources outlive any
release. A table with deletion protection and 35 days of point-in-time recovery
should not be in the same plan as a Lambda function that is replaced on every
deployment.

The table definitions arrive already decoded. Nothing under `infra/` reads a
file: the caller decodes `migrations/regional/generated/regional-tables.json`
and passes the result in, along with the digest the release manifest pins, so a
root planning against a stale bundle fails before it plans.

## Sanitized values

Every value is a variable with no default except `content_authority`. No account
id, ARN or domain appears in any `.tf` file here.

## Modules used

| Module | Purpose |
| --- | --- |
| `vpc-regional` | Network, endpoints, no NAT. |
| `kms-key` | One key per authority, instantiated with `for_each`. |
| `regional-dynamodb-tables` | Every table in the region. |
| `content-bucket` | Regional content store. |

## Outputs

`vpc_id`, `private_subnet_ids`, `public_subnet_ids`,
`interface_endpoint_security_group_id`, `gateway_endpoint_prefix_list_ids`,
`authority_key_arns`, `table_names`, `stream_arns`, `content_bucket`.

The last three network handles exist for the application root's security
groups: a task group egresses to the interface endpoints by group reference and
to S3 and DynamoDB by AWS-managed prefix list, because a gateway endpoint is a
route-table entry rather than an interface and cannot be named any other way.

## Test

`tests/region-foundation.tftest.hcl` plans the root against a mock AWS provider
and asserts the derived content bucket name, the pinned keystore name, the
absence of any NAT gateway, and that every authority a table names has a key.
