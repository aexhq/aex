locals {
  context_equals = {
    for k, v in var.encryption_context_equals : "kms:EncryptionContext:${k}" => v
  }

  key_policy = jsonencode({
    Version = "2012-10-17"
    Id      = "aex-kms-key-policy"
    Statement = [
      for s in var.policy_statements : merge(
        {
          Sid       = s.sid
          Effect    = s.effect
          Principal = { (s.principal_type) = s.principals }
          Action    = s.actions
          Resource  = s.resources
        },
        s.data_plane ? {
          Condition = {
            StringEquals = merge(
              local.context_equals,
              { "kms:EncryptionContext:aex:workspace" = s.encryption_context_workspace }
            )
          }
        } : {}
      )
    ]
  })
}

resource "aws_kms_key" "this" {
  description             = var.description
  enable_key_rotation     = var.rotation
  rotation_period_in_days = var.rotation_period_days
  deletion_window_in_days = var.deletion_window_days
  policy                  = local.key_policy
  tags                    = var.tags
}

resource "aws_kms_alias" "this" {
  name          = var.alias
  target_key_id = aws_kms_key.this.key_id
}
