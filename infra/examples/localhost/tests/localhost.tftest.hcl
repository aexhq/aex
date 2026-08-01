# This root creates nothing and declares no provider, so there is no provider to
# mock. Every other example root in this directory uses `mock_provider "aws" {}`;
# here the run blocks below are the whole of the plan gate.

run "the_localhost_root_plans" {
  command = plan

  assert {
    condition     = output.endpoints.dynamodb == "http://127.0.0.1:8000"
    error_message = "The DynamoDB Local endpoint must be built from the host and port."
  }

  assert {
    condition     = output.endpoints.s3 == "http://127.0.0.1:9000"
    error_message = "The MinIO endpoint must be built from the host and port."
  }

  assert {
    condition     = startswith(output.endpoints.postgres, "postgres://127.0.0.1:5432/")
    error_message = "The PostgreSQL endpoint must be built from the host, port and database."
  }
}

run "every_image_is_pinned_to_the_recorded_tag" {
  command = plan

  assert {
    condition     = output.images.dynamodb == "amazon/dynamodb-local:2.6.1"
    error_message = "DynamoDB Local must be pinned to 2.6.1."
  }

  assert {
    condition     = output.images.minio == "minio/minio:RELEASE.2025-04-22T22-12-26Z"
    error_message = "MinIO must be pinned to RELEASE.2025-04-22T22-12-26Z."
  }

  assert {
    condition     = output.images.postgres == "postgres:17.5-bookworm"
    error_message = "PostgreSQL must be pinned to 17.5-bookworm."
  }

  assert {
    condition = alltrue([
      for k, v in output.images : length(split(":", v)) == 2 && !endswith(v, ":latest")
    ])
    error_message = "No local substitute may float on a mutable tag."
  }
}

run "every_substitute_records_what_it_cannot_prove" {
  command = plan

  assert {
    condition = alltrue([
      for k, v in output.images : length(lookup(output.cannot_prove, k, "")) > 0
    ])
    error_message = "Every local substitute must record the semantics it cannot prove, so a green local run is never mistaken for evidence."
  }
}

run "rejects_an_unpinned_dynamodb_image" {
  command = plan

  variables {
    dynamodb_local_image = "amazon/dynamodb-local:latest"
  }

  expect_failures = [var.dynamodb_local_image]
}

run "rejects_an_unpinned_minio_image" {
  command = plan

  variables {
    minio_image = "minio/minio:latest"
  }

  expect_failures = [var.minio_image]
}

run "rejects_an_unpinned_postgres_image" {
  command = plan

  variables {
    postgres_image = "postgres:17"
  }

  expect_failures = [var.postgres_image]
}

run "rejects_binding_the_substrate_outside_loopback" {
  command = plan

  variables {
    host = "0.0.0.0"
  }

  expect_failures = [var.host]
}
