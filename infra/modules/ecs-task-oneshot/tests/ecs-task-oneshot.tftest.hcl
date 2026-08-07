mock_provider "aws" {}

variables {
  family             = "aex-dev-central-schema-admin"
  image              = "000000000000.dkr.ecr.eu-west-1.amazonaws.com/aex/central-schema-admin@sha256:0000000000000000000000000000000000000000000000000000000000000000"
  cpu                = 1024
  memory             = 2048
  role_arn           = "arn:aws:iam::000000000000:role/aex-dev-central-schema-admin"
  execution_role_arn = "arn:aws:iam::000000000000:role/aex-dev-ecs-execution"
  subnets            = ["subnet-0123456789abcdef0"]
  log_group_name     = "/aex/dev/central-schema-admin"
  region             = "eu-west-1"

  secret_env = {
    AEX_CENTRAL_ADMIN_SECRET = "arn:aws:secretsmanager:eu-west-1:000000000000:secret:aex-dev-central-admin"
  }
}

run "the_image_is_digest_pinned" {
  command = plan

  assert {
    condition     = jsondecode(aws_ecs_task_definition.this.container_definitions)[0].image == var.image
    error_message = "The container must run the digest-pinned image it was given."
  }

  assert {
    condition     = can(regex("@sha256:[0-9a-f]{64}$", jsondecode(aws_ecs_task_definition.this.container_definitions)[0].image))
    error_message = "The container image must be digest-pinned."
  }
}

run "the_runtime_platform_is_explicit" {
  command = plan

  assert {
    condition     = one(aws_ecs_task_definition.this.runtime_platform).cpu_architecture == "ARM64"
    error_message = "The runtime platform must be declared so an image index cannot pick a different child."
  }
}

run "no_public_address_is_ever_assigned" {
  command = plan

  assert {
    condition     = var.assign_public_ip == false
    error_message = "A one-shot task must not be given a public IP."
  }

  assert {
    condition     = output.network_configuration.assign_public_ip == false
    error_message = "The network configuration handed to RunTask must keep the public address off."
  }
}

run "secrets_are_referenced_not_inlined" {
  command = plan

  assert {
    condition = alltrue([
      for s in jsondecode(aws_ecs_task_definition.this.container_definitions)[0].secrets :
      startswith(s.valueFrom, "arn:")
    ])
    error_message = "Every secret must be an ARN reference."
  }
}

run "rejects_a_tag_reference" {
  command = plan

  variables {
    image = "000000000000.dkr.ecr.eu-west-1.amazonaws.com/aex/central-schema-admin:v1"
  }

  expect_failures = [var.image]
}

run "rejects_assigning_a_public_ip" {
  command = plan

  variables {
    assign_public_ip = true
  }

  expect_failures = [var.assign_public_ip]
}

run "rejects_a_plaintext_secret_value" {
  command = plan

  variables {
    secret_env = {
      AEX_CENTRAL_ADMIN_SECRET = "not-a-reference"
    }
  }

  expect_failures = [var.secret_env]
}
