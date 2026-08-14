mock_provider "aws" {}

variables {
  plane                       = "dev"
  region                      = "eu-west-1"
  name_prefix                 = "aex-dev-euw1-"
  content_bucket_purpose      = "workspace-files"
  content_authority           = "file"
  content_lifecycle_role_arn  = "arn:aws:iam::000000000000:role/aex-dev-session-maintenance-worker"
  table_definitions_digest    = "blake3:1111111111111111111111111111111111111111111111111111111111111111"
  expected_definitions_digest = "blake3:1111111111111111111111111111111111111111111111111111111111111111"

  vpc = {
    name               = "aex-dev-euw1"
    cidr               = "10.40.0.0/16"
    az_count           = 2
    availability_zones = ["eu-west-1a", "eu-west-1b"]
    endpoints          = ["ecr.api", "ecr.dkr", "kms", "logs", "secretsmanager", "sts", "sqs"]
    allow_nat          = true
    nat_justification  = "Brain Mux and Tool Mux call validated public provider and MCP HTTPS endpoints."
  }

  authority_keys = {
    authz   = { alias = "alias/aex-authz-dev-euw1", description = "Authz projection.", encryption_context_equals = { "aex:plane" = "dev" }, policy_statements = [{ sid = "Admin", effect = "Allow", principal_type = "AWS", principals = ["arn:aws:iam::000000000000:root"], actions = ["kms:Describe*"], resources = ["*"], data_plane = false }] }
    session = { alias = "alias/aex-session-dev-euw1", description = "Session authority.", encryption_context_equals = { "aex:plane" = "dev" }, policy_statements = [{ sid = "Admin", effect = "Allow", principal_type = "AWS", principals = ["arn:aws:iam::000000000000:root"], actions = ["kms:Describe*"], resources = ["*"], data_plane = false }] }
    work    = { alias = "alias/aex-work-dev-euw1", description = "Work authority.", encryption_context_equals = { "aex:plane" = "dev" }, policy_statements = [{ sid = "Admin", effect = "Allow", principal_type = "AWS", principals = ["arn:aws:iam::000000000000:root"], actions = ["kms:Describe*"], resources = ["*"], data_plane = false }] }
    runtime = { alias = "alias/aex-runtime-dev-euw1", description = "Runtime authority.", encryption_context_equals = { "aex:plane" = "dev" }, policy_statements = [{ sid = "Admin", effect = "Allow", principal_type = "AWS", principals = ["arn:aws:iam::000000000000:root"], actions = ["kms:Describe*"], resources = ["*"], data_plane = false }] }
    file    = { alias = "alias/aex-file-dev-euw1", description = "File authority.", encryption_context_equals = { "aex:plane" = "dev" }, policy_statements = [{ sid = "Admin", effect = "Allow", principal_type = "AWS", principals = ["arn:aws:iam::000000000000:root"], actions = ["kms:Describe*"], resources = ["*"], data_plane = false }] }
  }

  table_definitions = [
    { logical_name = "regional-authz-projection", authority = "authz", hash_key = "pk", range_key = "sk", billing_mode = "PAY_PER_REQUEST", point_in_time_recovery_days = 35, deletion_protection = true, attributes = [{ name = "pk", type = "S" }, { name = "sk", type = "S" }] },
    { logical_name = "session-authority", authority = "session", hash_key = "pk", range_key = "sk", billing_mode = "PAY_PER_REQUEST", point_in_time_recovery_days = 35, deletion_protection = true, attributes = [{ name = "pk", type = "S" }, { name = "sk", type = "S" }] },
    { logical_name = "regional-work", authority = "work", hash_key = "pk", range_key = "sk", billing_mode = "PAY_PER_REQUEST", point_in_time_recovery_days = 35, deletion_protection = true, attributes = [{ name = "pk", type = "S" }, { name = "sk", type = "S" }] },
    { logical_name = "runtime-activity", authority = "runtime", hash_key = "pk", range_key = "sk", billing_mode = "PAY_PER_REQUEST", point_in_time_recovery_days = 35, deletion_protection = true, attributes = [{ name = "pk", type = "S" }, { name = "sk", type = "S" }] },
    { logical_name = "regional-file-authority", authority = "file", hash_key = "pk", range_key = "sk", billing_mode = "PAY_PER_REQUEST", point_in_time_recovery_days = 35, deletion_protection = true, attributes = [{ name = "pk", type = "S" }, { name = "sk", type = "S" }] },
  ]
}

run "exact_regional_foundation" {
  command = plan

  assert {
    condition     = toset(keys(module.tables.table_names)) == toset(["regional-authz-projection", "session-authority", "regional-work", "runtime-activity", "regional-file-authority"])
    error_message = "The foundation must create exactly the five session-MVP tables."
  }

  assert {
    condition     = module.content.bucket == "aex-dev-eu-west-1-workspace-files"
    error_message = "Workspace files must use the declared encrypted file bucket namespace."
  }

  assert {
    condition     = length(module.network.nat_gateway_ids) == 2
    error_message = "Each private subnet needs a same-zone NAT route for provider and MCP HTTPS calls."
  }
}
