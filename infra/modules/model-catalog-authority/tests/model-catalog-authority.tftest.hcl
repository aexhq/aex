mock_provider "aws" {}

override_data {
  target = data.aws_iam_policy_document.publisher_trust
  values = {
    json = "{\"Version\":\"2012-10-17\",\"Statement\":[]}"
  }
}

override_data {
  target = data.aws_iam_policy_document.key
  values = {
    json = "{\"Version\":\"2012-10-17\",\"Statement\":[]}"
  }
}

override_data {
  target = data.aws_iam_policy_document.publisher
  values = {
    json = "{\"Version\":\"2012-10-17\",\"Statement\":[]}"
  }
}

variables {
  region            = "eu-west-1"
  account_id        = "000000000000"
  oidc_provider_arn = "arn:aws:iam::000000000000:oidc-provider/token.actions.githubusercontent.com"
  kms_administrator_principal_arns = [
    "arn:aws:iam::000000000000:role/aex-model-catalog-key-owner",
  ]
  logical_key_id = "launch-primary-2026"
}

run "creates_a_dedicated_protected_catalog_signer" {
  command = plan

  assert {
    condition     = aws_kms_key.publisher.customer_master_key_spec == "ECC_NIST_P256"
    error_message = "The catalog publisher must use a P-256 asymmetric KMS key."
  }

  assert {
    condition     = aws_kms_key.publisher.key_usage == "SIGN_VERIFY"
    error_message = "The catalog publisher key must only sign and verify."
  }

  assert {
    condition     = aws_kms_key.publisher.enable_key_rotation == false
    error_message = "Automatic rotation is unsupported for the asymmetric publisher key."
  }

  assert {
    condition     = aws_kms_alias.publisher.name == "alias/aex-model-catalog-publisher"
    error_message = "The catalog publisher key must use its dedicated alias."
  }

  assert {
    condition     = aws_iam_role.publisher.name == "aex-model-catalog-publisher"
    error_message = "The protected workflow must assume only the dedicated publisher role."
  }

}
