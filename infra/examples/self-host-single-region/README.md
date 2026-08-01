# `self-host-single-region`

One root that stands a single-region deployment up in your own AWS account,
against artifacts published from this repository.

It is deliberately smaller than the five lifecycle roots put together. A
self-hoster running one region does not need the account/plane/region split that
exists so a multi-region operator can apply the stateful and releasable halves
separately. What is here is the shape those roots would compose to.

## What you supply

Every value is a variable with no default. In particular you supply:

- the decoded regional table bundle and the digest the release manifest pins for
  it, because nothing under `infra/` reads a file;
- the key policies, so every principal that can use your keys is written down in
  your own configuration;
- the Lambda artifact key, object version and `ChecksumSHA256`, taken from the
  published artifact envelope.

Copy the artifact into the bucket this root creates, then plan. If the checksum
S3 reports does not match the digest you supplied, the plan fails rather than
deploying bytes nobody has identified.

## Sanitized values

No account id, ARN or domain appears in any `.tf` file here. Where an example is
needed, `000000000000` stands for an account id and `example.invalid` for a
hostname; both appear only in documentation and tests.

## Modules used

`vpc-regional`, `kms-key`, `regional-dynamodb-tables`, `content-bucket`,
`artifact-bucket`, `sns-ops-topic`, `iam-deployable-role`, `lambda-function`.

## Outputs

`vpc_id`, `table_names`, `content_bucket`, `artifact_bucket`, `ops_topic_arn`,
`session_api_alias_arn`.

## Test

`tests/self-host-single-region.tftest.hcl` plans the root against a mock AWS
provider and asserts the derived bucket names, the digest-addressed artifact key
and the absence of any NAT gateway.
