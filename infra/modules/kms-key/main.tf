locals {
  context_equals = {
    for k, v in var.encryption_context_equals : "kms:EncryptionContext:${k}" => v
  }

  # Every statement's explicit conditions, grouped the way IAM expects them:
  # one block per operator, one key per condition variable, and the values of
  # repeated (operator, variable) pairs merged rather than one silently winning.
  explicit_conditions = {
    for s in var.policy_statements : s.sid => {
      for test in distinct([for c in coalesce(s.conditions, []) : c.test]) :
      test => {
        for variable in distinct([for c in coalesce(s.conditions, []) : c.variable if c.test == test]) :
        variable => distinct(flatten([
          for c in coalesce(s.conditions, []) : c.values if c.test == test && c.variable == variable
        ]))
      }
    }
  }

  # The tenant condition on a data-plane grant. It is always `StringEquals`, so
  # it is merged into any `StringEquals` the statement already carries instead
  # of replacing it.
  data_plane_equals = {
    for s in var.policy_statements : s.sid => (
      s.data_plane
      ? merge(local.context_equals, { "kms:EncryptionContext:aex:workspace" = s.encryption_context_workspace })
      : {}
    )
  }

  conditions = {
    for s in var.policy_statements : s.sid => merge(
      local.explicit_conditions[s.sid],
      s.data_plane ? {
        StringEquals = merge(
          lookup(local.explicit_conditions[s.sid], "StringEquals", {}),
          local.data_plane_equals[s.sid],
        )
      } : {},
    )
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
        length(local.conditions[s.sid]) > 0 ? { Condition = local.conditions[s.sid] } : {}
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
