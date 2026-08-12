mock_provider "aws" {}

variables {
  cluster_identifier     = "aex-dev-central-finance"
  database_name          = "aex_finance"
  master_username        = "aex_admin"
  engine_version         = "17.5"
  subnet_ids             = ["subnet-0123456789abcdef0", "subnet-0123456789abcdef1"]
  vpc_security_group_ids = ["sg-0123456789abcdef0"]
  region                 = "eu-west-1"
  kms_key_arn            = "arn:aws:kms:eu-west-1:000000000000:key/00000000-0000-4000-8000-000000000000"

  final_snapshot_identifier = "aex-dev-central-finance-final"
}

run "backups_deletion_protection_and_the_data_api_are_all_on" {
  command = plan

  assert {
    condition     = aws_rds_cluster.this.backup_retention_period >= 7
    error_message = "Backup retention must be at least 7 days."
  }

  assert {
    condition     = aws_rds_cluster.this.deletion_protection == true
    error_message = "Deletion protection must be enabled."
  }

  assert {
    condition     = aws_rds_cluster.this.enable_http_endpoint == true
    error_message = "The Data API must be enabled."
  }

  assert {
    condition     = aws_rds_cluster.this.storage_encrypted == true
    error_message = "Storage must be encrypted."
  }
}

run "no_instance_is_publicly_accessible" {
  command = plan

  assert {
    condition     = aws_rds_cluster_instance.writer.publicly_accessible == false
    error_message = "The writer instance must not be publicly accessible."
  }
}

run "dev_can_omit_a_reader_instance" {
  command = plan

  assert {
    condition     = length(aws_rds_cluster_instance.reader) == 0
    error_message = "No reader instance may exist at launch."
  }
}

run "production_can_create_one_warm_failover_reader" {
  command = plan

  variables {
    reader_count = 1
  }

  assert {
    condition     = length(aws_rds_cluster_instance.reader) == 1
    error_message = "One warm failover reader must be legal for the production binding."
  }
}

run "serverless_scaling_is_bounded" {
  command = plan

  assert {
    condition     = one(aws_rds_cluster.this.serverlessv2_scaling_configuration).min_capacity == var.min_acu
    error_message = "The minimum capacity must be applied."
  }

  assert {
    condition     = one(aws_rds_cluster.this.serverlessv2_scaling_configuration).max_capacity == var.max_acu
    error_message = "The maximum capacity must be applied."
  }
}

run "the_optional_lambda_role_enables_the_exact_aurora_feature" {
  command = plan

  variables {
    lambda_invoke_role = {
      arn = "arn:aws:iam::000000000000:role/aex-dev-aurora-control-wake"
    }
  }

  assert {
    condition     = one(aws_rds_cluster_role_association.lambda_invoke).feature_name == "Lambda"
    error_message = "Aurora Lambda invocation must use the RDS Lambda feature association."
  }
}

run "omitting_the_lambda_role_omits_the_association" {
  command = plan

  assert {
    condition     = length(aws_rds_cluster_role_association.lambda_invoke) == 0
    error_message = "An absent Lambda invocation role must plan no cluster role association."
  }
}

run "rejects_an_invalid_lambda_invocation_role_arn" {
  command = plan

  variables {
    lambda_invoke_role = {
      arn = "not-an-iam-role-arn"
    }
  }

  expect_failures = [var.lambda_invoke_role]
}

run "the_master_password_is_managed_and_never_configured" {
  command = plan

  assert {
    condition     = aws_rds_cluster.this.manage_master_user_password == true
    error_message = "The master password must be minted and rotated by RDS, never written into configuration."
  }

  assert {
    condition     = aws_rds_cluster.this.master_password == null
    error_message = "No master password may appear in configuration or state."
  }
}

run "rejects_backup_retention_below_seven_days" {
  command = plan

  variables {
    backup_retention_days = 1
  }

  expect_failures = [var.backup_retention_days]
}

run "rejects_disabling_the_data_api" {
  command = plan

  variables {
    data_api_enabled = false
  }

  expect_failures = [var.data_api_enabled]
}

run "rejects_a_publicly_accessible_cluster" {
  command = plan

  variables {
    publicly_accessible = true
  }

  expect_failures = [var.publicly_accessible]
}

run "rejects_more_than_one_reader_instance" {
  command = plan

  variables {
    reader_count = 2
  }

  expect_failures = [var.reader_count]
}

run "rejects_an_unpinned_engine_version" {
  command = plan

  variables {
    engine_version = "17"
  }

  expect_failures = [var.engine_version]
}

run "the_default_lifecycle_takes_a_final_snapshot_rather_than_leaving_the_cluster_undeletable" {
  command = plan

  assert {
    condition     = aws_rds_cluster.this.skip_final_snapshot == false
    error_message = "The default must take a final snapshot, not skip it."
  }

  assert {
    condition     = aws_rds_cluster.this.final_snapshot_identifier == var.final_snapshot_identifier
    error_message = "The configured final snapshot identifier must reach the cluster; RDS rejects a delete that names neither a snapshot nor a skip."
  }
}

run "a_teardown_may_disable_deletion_protection_when_it_states_why" {
  command = plan

  variables {
    deletion_protection                 = false
    deletion_protection_override_reason = "The prd plane is being torn down prelaunch; the cluster holds no customer data."
  }

  assert {
    condition     = aws_rds_cluster.this.deletion_protection == false
    error_message = "A deliberate, justified override must be able to unprotect the cluster; otherwise the plane cannot be torn down at all."
  }
}

run "a_teardown_may_skip_the_final_snapshot_outright" {
  command = plan

  variables {
    skip_final_snapshot       = true
    final_snapshot_identifier = null
  }

  assert {
    condition     = aws_rds_cluster.this.skip_final_snapshot == true
    error_message = "Skipping the final snapshot must be expressible for a no-recovery teardown."
  }

  assert {
    condition     = aws_rds_cluster.this.final_snapshot_identifier == null
    error_message = "No snapshot identifier may be carried when the snapshot is skipped."
  }
}

run "rejects_disabling_deletion_protection_without_a_reason" {
  command = plan

  variables {
    deletion_protection = false
  }

  expect_failures = [var.deletion_protection]
}

run "rejects_a_blank_deletion_protection_override_reason" {
  command = plan

  variables {
    deletion_protection                 = false
    deletion_protection_override_reason = "   "
  }

  expect_failures = [var.deletion_protection]
}

run "rejects_a_missing_final_snapshot_identifier_when_the_snapshot_is_not_skipped" {
  command = plan

  variables {
    final_snapshot_identifier = null
  }

  expect_failures = [var.final_snapshot_identifier]
}

run "rejects_a_final_snapshot_identifier_alongside_a_skipped_snapshot" {
  command = plan

  variables {
    skip_final_snapshot       = true
    final_snapshot_identifier = "aex-dev-central-finance-final"
  }

  expect_failures = [var.final_snapshot_identifier]
}

run "rejects_a_final_snapshot_identifier_that_is_not_a_legal_rds_identifier" {
  command = plan

  variables {
    final_snapshot_identifier = "9-starts-with-a-digit"
  }

  expect_failures = [var.final_snapshot_identifier]
}
