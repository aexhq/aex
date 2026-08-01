resource "aws_db_subnet_group" "this" {
  name       = "${var.cluster_identifier}-subnets"
  subnet_ids = var.subnet_ids
  tags       = var.tags
}

resource "aws_rds_cluster" "this" {
  cluster_identifier     = var.cluster_identifier
  engine                 = "aurora-postgresql"
  engine_mode            = "provisioned"
  engine_version         = var.engine_version
  database_name          = var.database_name
  master_username        = var.master_username
  db_subnet_group_name   = aws_db_subnet_group.this.name
  vpc_security_group_ids = var.vpc_security_group_ids

  storage_encrypted = true
  kms_key_id        = var.kms_key_arn

  # The master password is minted and rotated by RDS. No password value ever
  # reaches configuration, state or a plan file.
  manage_master_user_password   = true
  master_user_secret_kms_key_id = var.kms_key_arn

  enable_http_endpoint    = var.data_api_enabled
  backup_retention_period = var.backup_retention_days
  preferred_backup_window = var.preferred_backup_window
  copy_tags_to_snapshot   = true
  deletion_protection     = var.deletion_protection

  serverlessv2_scaling_configuration {
    min_capacity = var.min_acu
    max_capacity = var.max_acu
  }

  tags = var.tags

  lifecycle {
    precondition {
      condition     = split(":", var.admin_secret_arn)[3] == var.region
      error_message = "The admin secret must live in the same region as the cluster; a cross-region secret would make the Data API call fail at runtime rather than at plan time."
    }
  }
}

resource "aws_rds_cluster_instance" "writer" {
  identifier          = "${var.cluster_identifier}-writer"
  cluster_identifier  = aws_rds_cluster.this.id
  instance_class      = "db.serverless"
  engine              = aws_rds_cluster.this.engine
  engine_version      = aws_rds_cluster.this.engine_version
  publicly_accessible = var.publicly_accessible
  tags                = var.tags
}

# There is no reader at launch. The count is driven by a variable the module
# validates to zero so the intent is explicit rather than an omission.
resource "aws_rds_cluster_instance" "reader" {
  count = var.reader_count

  identifier          = "${var.cluster_identifier}-reader-${count.index}"
  cluster_identifier  = aws_rds_cluster.this.id
  instance_class      = "db.serverless"
  engine              = aws_rds_cluster.this.engine
  engine_version      = aws_rds_cluster.this.engine_version
  publicly_accessible = var.publicly_accessible
  tags                = var.tags
}
