mock_provider "aws" {}

variables {
  name = "aex/brain-mux"

  lifecycle_by_reference = {
    untagged_expire_days = 14
  }
}

run "tag_immutability_is_enforced" {
  command = plan

  assert {
    condition     = aws_ecr_repository.this.image_tag_mutability == "IMMUTABLE"
    error_message = "Tag immutability must be enforced on the repository."
  }

  assert {
    condition     = one(aws_ecr_repository.this.image_scanning_configuration).scan_on_push == true
    error_message = "A pushed image must be scanned on push."
  }
}

run "lifecycle_expires_only_unreferenced_digests" {
  command = plan

  assert {
    condition = alltrue([
      for r in jsondecode(aws_ecr_lifecycle_policy.this.policy).rules :
      r.selection.tagStatus == "untagged"
    ])
    error_message = "Every lifecycle rule must select untagged digests only; a tagged image may still be named by a released manifest."
  }

  assert {
    condition = alltrue([
      for r in jsondecode(aws_ecr_lifecycle_policy.this.policy).rules :
      r.action.type == "expire" && r.selection.countNumber == var.lifecycle_by_reference.untagged_expire_days
    ])
    error_message = "The lifecycle rule must expire untagged digests after the configured window."
  }

  assert {
    condition     = length(jsondecode(aws_ecr_lifecycle_policy.this.policy).rules) == 1
    error_message = "There must be exactly one lifecycle rule so no second rule can reach a tagged image."
  }
}

run "rejects_mutable_tags" {
  command = plan

  variables {
    immutable_tags = false
  }

  expect_failures = [var.immutable_tags]
}

run "rejects_force_delete" {
  command = plan

  variables {
    force_delete = true
  }

  expect_failures = [var.force_delete]
}

run "rejects_a_repository_outside_the_aex_namespace" {
  command = plan

  variables {
    name = "brain-mux"
  }

  expect_failures = [var.name]
}

run "rejects_an_untagged_expiry_of_zero_days" {
  command = plan

  variables {
    lifecycle_by_reference = {
      untagged_expire_days = 0
    }
  }

  expect_failures = [var.lifecycle_by_reference]
}
