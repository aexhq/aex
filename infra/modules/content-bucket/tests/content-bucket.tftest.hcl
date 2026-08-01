mock_provider "aws" {}

variables {
  plane              = "dev"
  region             = "eu-west-1"
  purpose            = "content"
  kms_key_arn        = "arn:aws:kms:eu-west-1:000000000000:key/00000000-0000-4000-8000-000000000000"
  lifecycle_role_arn = "arn:aws:iam::000000000000:role/aex-content-lifecycle"
}

run "the_name_is_plane_qualified_and_names_what_it_holds" {
  command = plan

  assert {
    condition     = aws_s3_bucket.this.bucket == "aex-dev-eu-west-1-content"
    error_message = "The bucket name must be aex-<plane>-<region>-<purpose>."
  }
}

run "a_second_store_in_one_plane_carries_its_own_purpose" {
  command = plan

  variables {
    purpose = "observations"
  }

  assert {
    condition     = aws_s3_bucket.this.bucket == "aex-dev-eu-west-1-observations"
    error_message = "A second store in the same plane must not inherit another store's name."
  }
}

run "rejects_a_purpose_that_is_not_a_name_component" {
  command = plan

  variables {
    purpose = "Observations/2026"
  }

  expect_failures = [var.purpose]
}

run "versioning_is_disabled" {
  command = plan

  assert {
    condition     = aws_s3_bucket_versioning.this.versioning_configuration[0].status == "Disabled"
    error_message = "Content object keys are content-addressed; versioning must stay disabled."
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

run "bucket_key_is_on_with_the_customer_managed_key" {
  command = plan

  assert {
    condition     = one(aws_s3_bucket_server_side_encryption_configuration.this.rule).bucket_key_enabled
    error_message = "S3 Bucket Keys must be on so the KMS request rate stays bounded."
  }

  assert {
    condition     = one(one(aws_s3_bucket_server_side_encryption_configuration.this.rule).apply_server_side_encryption_by_default).sse_algorithm == "aws:kms"
    error_message = "Default encryption must be SSE-KMS."
  }

  assert {
    condition     = one(one(aws_s3_bucket_server_side_encryption_configuration.this.rule).apply_server_side_encryption_by_default).kms_master_key_id == var.kms_key_arn
    error_message = "Default encryption must name the supplied customer-managed key."
  }
}

run "incomplete_multipart_uploads_are_aborted_after_twenty_four_hours" {
  command = plan

  assert {
    condition     = one(one(aws_s3_bucket_lifecycle_configuration.this.rule).abort_incomplete_multipart_upload).days_after_initiation == 1
    error_message = "Incomplete multipart uploads must be aborted after 24 hours."
  }

  assert {
    condition     = one(aws_s3_bucket_lifecycle_configuration.this.rule).status == "Enabled"
    error_message = "The abort rule must be enabled."
  }
}

run "policy_denies_non_tls" {
  command = plan

  assert {
    condition = length([
      for s in jsondecode(aws_s3_bucket_policy.this.policy).Statement : s
      if s.Effect == "Deny" && try(s.Condition.Bool["aws:SecureTransport"], "") == "false"
    ]) == 1
    error_message = "The bucket policy must deny any request that is not over TLS."
  }
}

run "policy_denies_unconditional_create" {
  command = plan

  assert {
    condition = length([
      for s in jsondecode(aws_s3_bucket_policy.this.policy).Statement : s
      if s.Effect == "Deny" && try(s.Condition.Null["s3:if-none-match"], "") == "true"
    ]) == 1
    error_message = "The bucket policy must deny a PutObject that carries no s3:if-none-match precondition."
  }
}

run "policy_denies_the_wrong_encryption" {
  command = plan

  assert {
    condition = length([
      for s in jsondecode(aws_s3_bucket_policy.this.policy).Statement : s
      if s.Effect == "Deny" && try(s.Condition.StringNotEquals["s3:x-amz-server-side-encryption"], "") == "aws:kms"
    ]) == 1
    error_message = "The bucket policy must deny a write that is not SSE-KMS."
  }

  assert {
    condition = length([
      for s in jsondecode(aws_s3_bucket_policy.this.policy).Statement : s
      if s.Effect == "Deny" && try(s.Condition.StringNotEquals["s3:x-amz-server-side-encryption-aws-kms-key-id"], "") == var.kms_key_arn
    ]) == 1
    error_message = "The bucket policy must deny a write encrypted with any key but the content key."
  }
}

run "policy_denies_a_stale_signature" {
  command = plan

  assert {
    condition = length([
      for s in jsondecode(aws_s3_bucket_policy.this.policy).Statement : s
      if s.Effect == "Deny" && try(s.Condition.NumericGreaterThan["s3:signatureAge"], "") == "300000"
    ]) == 1
    error_message = "The bucket policy must deny a request whose signature is older than 300000 milliseconds."
  }
}

run "policy_denies_delete_by_every_principal_but_the_lifecycle_role" {
  command = plan

  assert {
    condition = length([
      for s in jsondecode(aws_s3_bucket_policy.this.policy).Statement : s
      if s.Effect == "Deny" && try(s.NotPrincipal.AWS, []) == [var.lifecycle_role_arn]
      && contains(s.Action, "s3:DeleteObject")
    ]) == 1
    error_message = "The bucket policy must deny delete to every principal except the lifecycle role."
  }
}

run "rejects_a_signature_age_ceiling_above_five_minutes" {
  command = plan

  variables {
    signature_age_ms = 900000
  }

  expect_failures = [var.signature_age_ms]
}

run "rejects_a_multipart_abort_window_other_than_twenty_four_hours" {
  command = plan

  variables {
    abort_incomplete_multipart_days = 7
  }

  expect_failures = [var.abort_incomplete_multipart_days]
}

run "rejects_an_aws_managed_key" {
  command = plan

  variables {
    kms_key_arn = "alias/aws/s3"
  }

  expect_failures = [var.kms_key_arn]
}

run "rejects_a_lifecycle_principal_that_is_not_a_role" {
  command = plan

  variables {
    lifecycle_role_arn = "*"
  }

  expect_failures = [var.lifecycle_role_arn]
}
