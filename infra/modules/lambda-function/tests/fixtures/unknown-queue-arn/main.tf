resource "terraform_data" "queue" {
  input = "arn:aws:sqs:eu-west-1:000000000000:aex-dev-control-wake-dlq"
}

module "subject" {
  source = "../../.."

  function_name           = "aex-dev-regional-session-api"
  artifact_bucket         = "aex-infra-artifacts-dev-0a1b2c3d"
  artifact_key            = "lambda/regional-session-api/e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855.zip"
  artifact_object_version = "aBcDeFgHiJkLmNoPqRsTuVwXyZ012345"
  artifact_sha256         = "47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU="
  memory_mb               = 512
  timeout_s               = 30
  role_arn                = "arn:aws:iam::000000000000:role/aex-dev-regional-session-api"
  log_retention_days      = 30

  async_failure_destination = {
    arn = terraform_data.queue.output
  }
}
