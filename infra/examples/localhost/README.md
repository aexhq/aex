# `localhost`

The local substrate the integration lane runs against: DynamoDB Local, MinIO and
PostgreSQL, started by `compose.yaml`.

Terraform creates nothing here. This root declares no provider and has no
resources. It exists so the endpoints and the image pins live in one place that
the same `terraform test` gate covers as everything else in `infra/`, and so the
list of things the substrate **cannot** prove is written down next to the
substrate itself rather than in a wiki nobody reads.

## Pinned images

| Service | Image |
| --- | --- |
| DynamoDB Local | `amazon/dynamodb-local:2.6.1` |
| MinIO | `minio/minio:RELEASE.2025-04-22T22-12-26Z` |
| PostgreSQL | `postgres:17.5-bookworm` |

`release/policy/test-images.toml` is the source of truth. The tags in
`compose.yaml` and the defaults in `variables.tf` mirror it, and the variable
validations reject any other value so the two cannot drift apart quietly.

## What this substrate cannot prove

A green local run is **never** evidence for a seam marked `requires_live = true`.

- **DynamoDB Local**: streams, TTL actually firing, adaptive capacity and
  hot-partition throttling, `TransactionConflict` under real contention, PITR,
  IAM.
- **MinIO**: bucket-policy denial of unconditional put and of delete,
  `s3:signatureAge`, SSE-KMS and bucket keys, AWS checksum semantics, lifecycle
  rules, presigned-URL expiry.
- **PostgreSQL**: Aurora Serverless v2 ACU scaling and cold resume, failover, the
  Data API transport, Aurora IAM auth, PITR and restore.

The `cannot_prove` output repeats this so a test harness can read it rather than
re-deriving it.

## Running it

```sh
docker compose -f infra/examples/localhost/compose.yaml up -d --wait
docker compose -f infra/examples/localhost/compose.yaml down -v
```

Every service binds to loopback, runs its data directory on `tmpfs` and has a
health check, so the lane can wait for readiness instead of sleeping and each
run starts from an empty store.

## Test

`tests/localhost.tftest.hcl` plans the root and asserts the derived endpoints,
the exact image pins, that no image floats on a mutable tag, that every
substitute records what it cannot prove, and that binding outside loopback is
rejected. It is the one root with no `mock_provider` block, because it declares
no provider to mock.
