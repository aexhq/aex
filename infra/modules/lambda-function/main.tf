# The artifact is read, never built. Terraform packages no code: the ZIP was
# compiled, packaged, digested and published by the release lane, and this data
# source only confirms that the exact bytes the manifest pins are the bytes S3
# is holding.
data "aws_s3_object" "artifact" {
  bucket     = var.artifact_bucket
  key        = var.artifact_key
  version_id = var.artifact_object_version

  lifecycle {
    postcondition {
      condition     = self.checksum_sha256 == var.artifact_sha256
      error_message = "The stored artifact checksum does not match the digest the release manifest pins."
    }
  }
}

resource "aws_cloudwatch_log_group" "this" {
  name              = "/aws/lambda/${var.function_name}"
  retention_in_days = var.log_retention_days
  kms_key_id        = var.log_kms_key_arn
  tags              = var.tags
}

resource "aws_lambda_function" "this" {
  function_name = var.function_name
  role          = var.role_arn

  s3_bucket         = var.artifact_bucket
  s3_key            = var.artifact_key
  s3_object_version = var.artifact_object_version

  runtime       = var.runtime
  handler       = var.handler
  architectures = [var.architecture]
  memory_size   = var.memory_mb
  timeout       = var.timeout_s
  publish       = true

  reserved_concurrent_executions = var.reserved_concurrency
  code_signing_config_arn        = var.code_signing_config_arn

  dynamic "environment" {
    for_each = length(var.env) > 0 ? [var.env] : []

    content {
      variables = environment.value
    }
  }

  logging_config {
    log_format = "JSON"
    log_group  = aws_cloudwatch_log_group.this.name
  }

  tags = var.tags

  lifecycle {
    precondition {
      condition     = data.aws_s3_object.artifact.checksum_sha256 == var.artifact_sha256
      error_message = "The stored artifact checksum does not match the digest the release manifest pins; refusing to deploy unidentified bytes."
    }
  }
}

resource "aws_lambda_alias" "this" {
  name             = var.alias_name
  function_name    = aws_lambda_function.this.function_name
  function_version = aws_lambda_function.this.version
}

# Public function URLs are opt-in and alias-qualified. With AWS provider 6.x,
# `authorization_type = "NONE"` creates the two public resource-policy
# statements AWS requires: InvokeFunctionUrl is conditioned on auth type NONE,
# and InvokeFunction is conditioned on InvokedViaFunctionUrl. Qualifying this
# resource with the immutable alias keeps both statements off the unqualified
# function and prevents direct public InvokeFunction access.
resource "aws_lambda_function_url" "public" {
  count = var.public_function_url_enabled ? 1 : 0

  function_name      = aws_lambda_function.this.function_name
  qualifier          = aws_lambda_alias.this.name
  authorization_type = "NONE"
  invoke_mode        = "BUFFERED"
}

resource "aws_lambda_function_event_invoke_config" "this" {
  count = var.async_failure_destination == null ? 0 : 1

  function_name                = aws_lambda_function.this.function_name
  qualifier                    = aws_lambda_alias.this.name
  maximum_event_age_in_seconds = var.async_max_event_age_seconds
  maximum_retry_attempts       = var.async_max_retry_attempts

  destination_config {
    on_failure {
      destination = var.async_failure_destination.arn
    }
  }
}
