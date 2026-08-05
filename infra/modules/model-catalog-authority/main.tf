data "aws_partition" "current" {}

locals {
  partition = data.aws_partition.current.partition
  role_name = "aex-model-catalog-publisher"
  key_alias = "alias/aex-model-catalog-publisher"
  base_tags = merge(var.tags, {
    authority  = "model-catalog-publisher"
    managed_by = "terraform"
    region     = var.region
  })
}

data "aws_iam_policy_document" "publisher_trust" {
  statement {
    sid     = "AssumeFromExactProtectedPublisher"
    effect  = "Allow"
    actions = ["sts:AssumeRoleWithWebIdentity"]
    principals {
      type        = "Federated"
      identifiers = [var.oidc_provider_arn]
    }
    condition {
      test     = "StringEquals"
      variable = "token.actions.githubusercontent.com:aud"
      values   = ["sts.amazonaws.com"]
    }
    condition {
      test     = "StringEquals"
      variable = "token.actions.githubusercontent.com:sub"
      values   = ["repo:${var.repository}:environment:${var.environment}"]
    }
    condition {
      test     = "StringEquals"
      variable = "token.actions.githubusercontent.com:repository"
      values   = [var.repository]
    }
    condition {
      test     = "StringEquals"
      variable = "token.actions.githubusercontent.com:ref"
      values   = ["refs/heads/main"]
    }
    condition {
      test     = "StringEquals"
      variable = "token.actions.githubusercontent.com:workflow"
      values   = [var.workflow_name]
    }
  }
}

resource "aws_iam_role" "publisher" {
  name                 = local.role_name
  assume_role_policy   = data.aws_iam_policy_document.publisher_trust.json
  max_session_duration = 3600
  tags                 = local.base_tags
}

data "aws_iam_policy_document" "key" {
  statement {
    sid    = "AdministerByExactOwnerPrincipals"
    effect = "Allow"
    principals {
      type        = "AWS"
      identifiers = var.kms_administrator_principal_arns
    }
    actions = [
      "kms:CancelKeyDeletion",
      "kms:CreateAlias",
      "kms:DeleteAlias",
      "kms:DescribeKey",
      "kms:DisableKey",
      "kms:EnableKey",
      "kms:GetKeyPolicy",
      "kms:ListGrants",
      "kms:ListKeyPolicies",
      "kms:ListResourceTags",
      "kms:PutKeyPolicy",
      "kms:RevokeGrant",
      "kms:ScheduleKeyDeletion",
      "kms:TagResource",
      "kms:UntagResource",
      "kms:UpdateAlias",
      "kms:UpdateKeyDescription",
    ]
    resources = ["*"]
  }

  statement {
    sid    = "InspectByExactPublisher"
    effect = "Allow"
    principals {
      type        = "AWS"
      identifiers = [aws_iam_role.publisher.arn]
    }
    actions   = ["kms:DescribeKey", "kms:GetPublicKey"]
    resources = ["*"]
  }

  statement {
    sid    = "SignEcdsaSha256ByExactPublisher"
    effect = "Allow"
    principals {
      type        = "AWS"
      identifiers = [aws_iam_role.publisher.arn]
    }
    actions   = ["kms:Sign"]
    resources = ["*"]
    condition {
      test     = "StringEquals"
      variable = "kms:SigningAlgorithm"
      values   = ["ECDSA_SHA_256"]
    }
  }
}

resource "aws_kms_key" "publisher" {
  description                        = "Dedicated AEX model-catalog signing authority"
  customer_master_key_spec           = "ECC_NIST_P256"
  key_usage                          = "SIGN_VERIFY"
  enable_key_rotation                = false
  multi_region                       = false
  policy                             = data.aws_iam_policy_document.key.json
  bypass_policy_lockout_safety_check = false
  deletion_window_in_days            = 30
  tags                               = local.base_tags

  lifecycle {
    prevent_destroy = true
  }
}

resource "aws_kms_alias" "publisher" {
  name          = local.key_alias
  target_key_id = aws_kms_key.publisher.key_id
}

data "aws_iam_policy_document" "publisher" {
  statement {
    sid       = "InspectExactCatalogKey"
    effect    = "Allow"
    actions   = ["kms:DescribeKey", "kms:GetPublicKey"]
    resources = [aws_kms_key.publisher.arn]
  }

  statement {
    sid       = "SignExactCatalogKeyWithEcdsaSha256"
    effect    = "Allow"
    actions   = ["kms:Sign"]
    resources = [aws_kms_key.publisher.arn]
    condition {
      test     = "StringEquals"
      variable = "kms:SigningAlgorithm"
      values   = ["ECDSA_SHA_256"]
    }
  }
}

resource "aws_iam_role_policy" "publisher" {
  name   = "${local.role_name}-access"
  role   = aws_iam_role.publisher.id
  policy = data.aws_iam_policy_document.publisher.json
}
