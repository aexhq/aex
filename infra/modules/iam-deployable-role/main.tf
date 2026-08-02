locals {
  role_name = "aex-${var.plane}-${var.deployable}"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid       = "AssumeByDeployable"
        Effect    = "Allow"
        Principal = { (var.assume_principal.type) = var.assume_principal.identifiers }
        Action    = "sts:AssumeRole"
      },
    ]
  })

  inline_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      for g in var.action_grants : merge(
        {
          Sid      = g.sid
          Effect   = "Allow"
          Action   = g.actions
          Resource = g.resources
        },
        g.scopable ? {
          Condition = {
            (g.condition_operator) = { (g.condition_key) = g.condition_values }
          }
        } : {}
      )
    ]
  })
}

resource "aws_iam_role" "this" {
  name                 = local.role_name
  assume_role_policy   = local.assume_role_policy
  permissions_boundary = var.boundary_policy_arn
  max_session_duration = var.max_session_duration

  tags = merge(var.tags, {
    "aex:plane"      = var.plane
    "aex:region"     = var.region
    "aex:deployable" = var.deployable
  })
}

resource "aws_iam_role_policy" "this" {
  name   = "${local.role_name}-inline"
  role   = aws_iam_role.this.id
  policy = local.inline_policy
}
