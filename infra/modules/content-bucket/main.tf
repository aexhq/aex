locals {
  bucket_name = "aex-${var.plane}-${var.region}-${var.purpose}"
  bucket_arn  = "arn:${var.partition}:s3:::${local.bucket_name}"
  object_arn  = "arn:${var.partition}:s3:::${local.bucket_name}/*"

  bucket_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid       = "DenyInsecureTransport"
        Effect    = "Deny"
        Principal = "*"
        Action    = "s3:*"
        Resource  = [local.bucket_arn, local.object_arn]
        Condition = { Bool = { "aws:SecureTransport" = "false" } }
      },
      {
        Sid       = "DenyUnconditionalCreate"
        Effect    = "Deny"
        Principal = "*"
        Action    = "s3:PutObject"
        Resource  = local.object_arn
        Condition = { Null = { "s3:if-none-match" = "true" } }
      },
      {
        Sid       = "DenyWrongEncryptionAlgorithm"
        Effect    = "Deny"
        Principal = "*"
        Action    = "s3:PutObject"
        Resource  = local.object_arn
        Condition = { StringNotEquals = { "s3:x-amz-server-side-encryption" = "aws:kms" } }
      },
      {
        Sid       = "DenyWrongEncryptionKey"
        Effect    = "Deny"
        Principal = "*"
        Action    = "s3:PutObject"
        Resource  = local.object_arn
        Condition = { StringNotEquals = { "s3:x-amz-server-side-encryption-aws-kms-key-id" = var.kms_key_arn } }
      },
      {
        Sid       = "DenyStaleSignature"
        Effect    = "Deny"
        Principal = "*"
        Action    = "s3:*"
        Resource  = [local.bucket_arn, local.object_arn]
        Condition = { NumericGreaterThan = { "s3:signatureAge" = tostring(var.signature_age_ms) } }
      },
      merge(
        {
          Sid      = var.lifecycle_role_arn == null ? "DenyDeleteAll" : "DenyDeleteExceptLifecycleRole"
          Effect   = "Deny"
          Action   = ["s3:DeleteObject", "s3:DeleteObjectVersion"]
          Resource = local.object_arn
        },
        var.lifecycle_role_arn == null
        ? { Principal = "*" }
        : { NotPrincipal = { AWS = [var.lifecycle_role_arn] } }
      ),
    ]
  })
}

resource "aws_s3_bucket" "this" {
  bucket = local.bucket_name
  tags   = var.tags
}

resource "aws_s3_bucket_versioning" "this" {
  bucket = aws_s3_bucket.this.id

  versioning_configuration {
    status = "Disabled"
  }
}

resource "aws_s3_bucket_public_access_block" "this" {
  bucket = aws_s3_bucket.this.id

  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

resource "aws_s3_bucket_server_side_encryption_configuration" "this" {
  bucket = aws_s3_bucket.this.id

  rule {
    bucket_key_enabled = true

    apply_server_side_encryption_by_default {
      sse_algorithm     = "aws:kms"
      kms_master_key_id = var.kms_key_arn
    }
  }
}

resource "aws_s3_bucket_cors_configuration" "browser" {
  count = length(var.browser_cors_origins) == 0 ? 0 : 1

  bucket = aws_s3_bucket.this.id

  cors_rule {
    allowed_headers = ["Content-Length", "Range", "x-amz-checksum-sha256", "x-amz-expected-bucket-owner"]
    allowed_methods = ["GET", "HEAD", "PUT"]
    allowed_origins = var.browser_cors_origins
    expose_headers  = ["Accept-Ranges", "Content-Length", "Content-Range", "ETag", "x-amz-checksum-sha256"]
    max_age_seconds = 300
  }
}

resource "aws_s3_bucket_lifecycle_configuration" "this" {
  bucket = aws_s3_bucket.this.id

  rule {
    id     = "abort-incomplete-multipart-uploads"
    status = "Enabled"

    filter {}

    abort_incomplete_multipart_upload {
      days_after_initiation = var.abort_incomplete_multipart_days
    }
  }
}

resource "aws_s3_bucket_policy" "this" {
  bucket = aws_s3_bucket.this.id
  policy = local.bucket_policy

  depends_on = [aws_s3_bucket_public_access_block.this]
}
