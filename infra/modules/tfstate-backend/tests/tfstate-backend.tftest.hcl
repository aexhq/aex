mock_provider "aws" {}

variables {
  plane              = "dev"
  bucket_name_suffix = "0a1b2c3d"
  kms_alias          = "alias/aex-tfstate-dev"
}

run "state_bucket_is_versioned" {
  command = plan

  assert {
    condition     = aws_s3_bucket_versioning.this.versioning_configuration[0].status == "Enabled"
    error_message = "The state bucket must be versioned so a corrupt state file can be recovered."
  }

  assert {
    condition     = one(one(aws_s3_bucket_lifecycle_configuration.this.rule).noncurrent_version_expiration).noncurrent_days >= 30
    error_message = "Superseded state versions must be kept for at least 30 days."
  }
}

run "public_access_is_blocked_on_all_four_settings" {
  command = plan

  assert {
    condition = alltrue([
      aws_s3_bucket_public_access_block.this.block_public_acls,
      aws_s3_bucket_public_access_block.this.block_public_policy,
      aws_s3_bucket_public_access_block.this.ignore_public_acls,
      aws_s3_bucket_public_access_block.this.restrict_public_buckets,
    ])
    error_message = "All four public-access-block settings must be on."
  }
}

run "policy_denies_non_tls" {
  command = plan

  assert {
    condition = length([
      for s in jsondecode(aws_s3_bucket_policy.this.policy).Statement : s
      if s.Effect == "Deny" && try(s.Condition.Bool["aws:SecureTransport"], "") == "false"
    ]) == 1
    error_message = "The state bucket policy must deny any request that is not over TLS."
  }
}

run "state_is_encrypted_with_a_rotating_customer_managed_key" {
  command = plan

  assert {
    condition     = aws_kms_key.state.enable_key_rotation == true
    error_message = "The state key must rotate."
  }

  assert {
    condition     = aws_kms_key.state.deletion_window_in_days >= 30
    error_message = "The state key deletion window must be at least 30 days."
  }

  assert {
    condition     = one(one(aws_s3_bucket_server_side_encryption_configuration.this.rule).apply_server_side_encryption_by_default).sse_algorithm == "aws:kms"
    error_message = "State must be encrypted with SSE-KMS."
  }

  assert {
    condition     = aws_kms_alias.state.name == var.kms_alias
    error_message = "The state key alias must be the configured alias."
  }
}

run "rejects_a_short_deletion_window" {
  command = plan

  variables {
    deletion_window_days = 7
  }

  expect_failures = [var.deletion_window_days]
}

run "rejects_an_alias_outside_the_aex_namespace" {
  command = plan

  variables {
    kms_alias = "alias/terraform"
  }

  expect_failures = [var.kms_alias]
}

run "rejects_a_short_superseded_state_retention" {
  command = plan

  variables {
    noncurrent_retention_days = 7
  }

  expect_failures = [var.noncurrent_retention_days]
}
