mock_provider "aws" {}

variables {
  plane                       = "dev"
  region                      = "eu-west-1"
  name_prefix                 = "aex-dev-euw1-"
  table_definitions_digest    = "blake3:1111111111111111111111111111111111111111111111111111111111111111"
  expected_definitions_digest = "blake3:1111111111111111111111111111111111111111111111111111111111111111"
  kms_key_arn_by_authority = {
    authz   = "arn:aws:kms:eu-west-1:000000000000:key/00000000-0000-4000-8000-000000000000"
    session = "arn:aws:kms:eu-west-1:000000000000:key/00000000-0000-4000-8000-000000000001"
    work    = "arn:aws:kms:eu-west-1:000000000000:key/00000000-0000-4000-8000-000000000002"
    runtime = "arn:aws:kms:eu-west-1:000000000000:key/00000000-0000-4000-8000-000000000003"
    file    = "arn:aws:kms:eu-west-1:000000000000:key/00000000-0000-4000-8000-000000000004"
  }
  table_definitions = [
    { logical_name = "regional-authz-projection", authority = "authz", hash_key = "pk", range_key = "sk", billing_mode = "PAY_PER_REQUEST", point_in_time_recovery_days = 35, deletion_protection = true, attributes = [{ name = "pk", type = "S" }, { name = "sk", type = "S" }] },
    { logical_name = "session-authority", authority = "session", hash_key = "pk", range_key = "sk", billing_mode = "PAY_PER_REQUEST", point_in_time_recovery_days = 35, deletion_protection = true, ttl_attribute = "expiresAtEpochSeconds", stream_view_type = "KEYS_ONLY", attributes = [{ name = "pk", type = "S" }, { name = "sk", type = "S" }] },
    { logical_name = "regional-work", authority = "work", hash_key = "pk", range_key = "sk", billing_mode = "PAY_PER_REQUEST", point_in_time_recovery_days = 35, deletion_protection = true, ttl_attribute = "expiresAtEpochSeconds", stream_view_type = "NEW_IMAGE", attributes = [{ name = "pk", type = "S" }, { name = "sk", type = "S" }] },
    { logical_name = "runtime-activity", authority = "runtime", hash_key = "pk", range_key = "sk", billing_mode = "PAY_PER_REQUEST", point_in_time_recovery_days = 35, deletion_protection = true, ttl_attribute = "expiresAtEpochSeconds", stream_view_type = "NEW_IMAGE", attributes = [{ name = "pk", type = "S" }, { name = "sk", type = "S" }] },
    { logical_name = "regional-file-authority", authority = "file", hash_key = "pk", range_key = "sk", billing_mode = "PAY_PER_REQUEST", point_in_time_recovery_days = 35, deletion_protection = true, ttl_attribute = "expiresAtEpochSeconds", stream_view_type = "NEW_IMAGE", attributes = [{ name = "pk", type = "S" }, { name = "sk", type = "S" }] },
  ]
}

run "exact_five_table_topology" {
  command = plan

  assert {
    condition     = toset(keys(aws_dynamodb_table.this)) == toset(["regional-authz-projection", "session-authority", "regional-work", "runtime-activity", "regional-file-authority"])
    error_message = "Regional storage must contain exactly the five session-MVP tables."
  }

  assert {
    condition = alltrue([for table in values(aws_dynamodb_table.this) :
      table.billing_mode == "PAY_PER_REQUEST"
      && table.deletion_protection_enabled
      && table.point_in_time_recovery[0].enabled
      && table.point_in_time_recovery[0].recovery_period_in_days >= 35
      && table.server_side_encryption[0].enabled
    ])
    error_message = "Every retained table must be on-demand, deletion protected, recoverable, and KMS encrypted."
  }
}
