resource "terraform_data" "role" {
  input = "arn:aws:iam::000000000000:role/aex-dev-aurora-control-wake"
}

module "subject" {
  source = "../../.."

  cluster_identifier     = "aex-dev-central-finance"
  database_name          = "aex_finance"
  master_username        = "aex_admin"
  engine_version         = "17.5"
  subnet_ids             = ["subnet-0123456789abcdef0", "subnet-0123456789abcdef1"]
  vpc_security_group_ids = ["sg-0123456789abcdef0"]
  region                 = "eu-west-1"
  kms_key_arn            = "arn:aws:kms:eu-west-1:000000000000:key/00000000-0000-4000-8000-000000000000"

  final_snapshot_identifier = "aex-dev-central-finance-final"
  lambda_invoke_role = {
    arn = terraform_data.role.output
  }
}
