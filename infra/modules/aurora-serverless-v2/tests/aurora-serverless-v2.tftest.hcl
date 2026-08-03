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

run "rejects_disabling_deletion_protection" {
  command = plan

  variables {
    deletion_protection = false
  }

  expect_failures = [var.deletion_protection]
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
