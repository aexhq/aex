locals {
  role_name = "aex-gha-${replace(var.repository, "/", "-")}-${var.permissions_profile}"

  # The subject names the exact repository and the exact ref. The job workflow
  # reference names the exact workflow file as well, so a new workflow in the
  # same repository on the same branch still cannot assume this role.
  allowed_subjects = [
    for r in var.allowed_refs : "repo:${var.repository}:ref:${r}"
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
      {
        Sid      = "ProfileGrants"
        Effect   = "Allow"
        Action   = var.permission_profiles[var.permissions_profile]
        Resource = var.profile_resources[var.permissions_profile]
      },
    ]
  })
}

resource "aws_iam_role" "this" {
  name                 = local.role_name
  assume_role_policy   = local.assume_role_policy
  max_session_duration = var.max_session_duration

  tags = merge(var.tags, {
    "aex:profile" = var.permissions_profile
  })
}

resource "aws_iam_role_policy" "this" {
  name   = "${local.role_name}-inline"
  role   = aws_iam_role.this.id
  policy = local.inline_policy
}
