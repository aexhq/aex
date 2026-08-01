# The local substrate is started by `compose.yaml`, not by Terraform. Terraform
# packages no code and starts no container: this root exists so the integration
# lane has one place to read the endpoints and the image pins from, and so those
# pins are covered by the same `terraform test` gate as everything else.
#
# There is no provider here and nothing is created. That is deliberate.
locals {
  images = {
    dynamodb = var.dynamodb_local_image
    minio    = var.minio_image
    postgres = var.postgres_image
  }

  endpoints = {
    dynamodb = "http://${var.host}:${var.dynamodb_port}"
    s3       = "http://${var.host}:${var.minio_port}"
    postgres = "postgres://${var.host}:${var.postgres_port}/${var.postgres_database}"
  }
}
