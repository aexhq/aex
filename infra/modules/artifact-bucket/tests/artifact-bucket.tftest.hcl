mock_provider "aws" {}

variables {
  plane              = "dev"
  bucket_name_suffix = "0a1b2c3d"
  retention_days     = 90
}

run "versioning_is_enabled" {
  command = plan

  assert {
    condition     = aws_s3_bucket_versioning.this.versioning_configuration[0].status == "Enabled"
    error_message = "The artifact bucket must be versioned so a bad infrastructure artifact can be rolled back."
  }
}

run "object_lock_is_off" {
  command = plan

  assert {
    condition     = aws_s3_bucket.this.object_lock_enabled == false
    error_message = "Object Lock must stay off on the artifact bucket."
  }
}

run "lifecycle_expires_noncurrent_versions" {
  command = plan

  assert {
    condition     = one(one(aws_s3_bucket_lifecycle_configuration.this.rule).noncurrent_version_expiration).noncurrent_days == var.retention_days
    error_message = "Noncurrent versions must expire after the configured retention."
  }

  assert {
    condition     = one(aws_s3_bucket_lifecycle_configuration.this.rule).status == "Enabled"
    error_message = "The noncurrent-expiry rule must be enabled."
  }
}

run "public_access_is_blocked_and_non_tls_is_denied" {
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

  assert {
    condition = length([
      for s in jsondecode(aws_s3_bucket_policy.this.policy).Statement : s
      if s.Effect == "Deny" && try(s.Condition.Bool["aws:SecureTransport"], "") == "false"
    ]) == 1
    error_message = "The bucket policy must deny any request that is not over TLS."
  }
}

run "rejects_enabling_object_lock" {
  command = plan

  variables {
    object_lock_enabled = true
  }

  expect_failures = [var.object_lock_enabled]
}

run "rejects_a_retention_of_zero_days" {
  command = plan

  variables {
    retention_days = 0
  }

  expect_failures = [var.retention_days]
}
