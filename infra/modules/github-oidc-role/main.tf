locals {
  # The subject names the exact repository and the exact ref. The job workflow
  # reference names the exact workflow file as well, so a new workflow in the
  # same repository on the same branch still cannot assume this role.
  # GitHub changes the standard `sub` claim from `ref:...` to
  # `environment:...` when a job uses a protected Environment. An environment-
  # bound role must accept only that subject; also accepting the ref subject
  # would let the same workflow assume it from a job that bypassed approval.
  allowed_subjects = length(var.allowed_environments) > 0 ? [
    for environment in var.allowed_environments : "repo:${var.repository}:environment:${environment}"
    ] : [
    for ref in var.allowed_refs : "repo:${var.repository}:ref:${ref}"
  ]

  allowed_job_workflow_refs = flatten([
    for w in var.allowed_workflows : [
      for r in var.allowed_refs : "${var.repository}/${w}@${r}"
    ]
  ])

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid       = "AssumeByGitHubActions"
        Effect    = "Allow"
        Principal = { Federated = var.oidc_provider_arn }
        Action    = "sts:AssumeRoleWithWebIdentity"
        Condition = {
          StringEquals = {
            "token.actions.githubusercontent.com:aud"              = "sts.amazonaws.com"
            "token.actions.githubusercontent.com:sub"              = local.allowed_subjects
            "token.actions.githubusercontent.com:job_workflow_ref" = local.allowed_job_workflow_refs
          }
        }
      },
    ]
  })

  inline_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      for statement in var.profile_statements[var.permissions_profile] : {
        Sid      = statement.sid
        Effect   = "Allow"
        Action   = statement.actions
        Resource = statement.resources
      }
    ]
  })
}

resource "aws_iam_role" "this" {
  name                 = var.role_name
  assume_role_policy   = local.assume_role_policy
  max_session_duration = var.max_session_duration

  tags = merge(var.tags, {
    "aex:profile" = var.permissions_profile
  })
}

resource "aws_iam_role_policy" "this" {
  name   = "${var.role_name}-inline"
  role   = aws_iam_role.this.id
  policy = local.inline_policy
}
