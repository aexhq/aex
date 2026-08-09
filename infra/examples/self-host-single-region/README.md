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
- the digest-pinned session API image, taken from the published artifact
  envelope, plus the ECS execution role.

You no longer supply a security group. `ecs-service` creates the group its tasks
run with, in the VPC this root stands up, and wires its egress to the interface
endpoints and the S3 and DynamoDB prefix lists that same network already
creates. That is the whole of what a task can reach: there is no NAT gateway and
no route to the internet, so an unnamed destination is unreachable rather than
merely unauthorised.

The image is pinned by digest, never by tag. A service that follows a tag can
restart onto different bytes with no deployment and no receipt, and the module
rejects a `:tag` reference rather than accepting one.

## No public edge

`regional-session-api` runs here as a Fargate service, and this root stands up
no ingress in front of it - exactly as it previously stood the function up with
no API gateway. A self-hoster supplies their own edge. `infra/examples/region-application`
is the worked example of the public load balancer, its listener and the
per-service target groups.

Because there is no edge, the task group admits nothing. Your load balancer
reaches the tasks when you write one ingress rule naming
`session_service_security_group_id`, next to whatever edge you put in front. The
group's rules are separate resources rather than inline blocks, so a rule added
from your own configuration is not drift.

## Sanitized values

No account id, ARN or domain appears in any `.tf` file here. Where an example is
needed, `000000000000` stands for an account id and `example.invalid` for a
hostname; both appear only in documentation and tests.

## Modules used

`vpc-regional`, `kms-key`, `regional-dynamodb-tables`, `content-bucket`,
`artifact-bucket`, `sns-ops-topic`, `iam-deployable-role`, `ecs-cluster`,
`log-group`, `ecs-service`.

## Outputs

`vpc_id`, `table_names`, `content_bucket`, `artifact_bucket`, `ops_topic_arn`,
`session_service_arn`, `session_service_security_group_id`, `cluster_arn`.

## Test

`tests/self-host-single-region.tftest.hcl` plans the root against a mock AWS
provider and asserts the derived bucket names, the digest-pinned service image,
that the cluster and log group are created here, that the session role is
assumed by `ecs-tasks.amazonaws.com`, and the absence of any NAT gateway.
