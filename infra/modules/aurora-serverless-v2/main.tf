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

  # RDS refuses a cluster delete that names neither a final snapshot nor a
  # deliberate skip. Expressing both is what makes this module's lifecycle
  # complete: without them the cluster is not merely protected, it is
  # undeletable, and the only remaining exit is to abandon it outside Terraform.
  # Both are Terraform-only attributes read from state at delete time, so a
  # change here has to be applied before the destroy that consumes it.
  skip_final_snapshot       = var.skip_final_snapshot
  final_snapshot_identifier = var.final_snapshot_identifier

  serverlessv2_scaling_configuration {
    min_capacity = var.min_acu
    max_capacity = var.max_acu
  }

  tags = var.tags

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

# Dev may omit the reader. Production may create one warm failover target; no
# application read path is directed at the reader endpoint.
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
