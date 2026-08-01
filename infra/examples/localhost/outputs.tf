output "endpoints" {
  value       = local.endpoints
  description = "Endpoint per local substitute, in the same shape the integration lane expects."
}

output "images" {
  value       = local.images
  description = "The pinned image reference for each local substitute. These must match `compose.yaml` and `release/policy/test-images.toml`."
}

output "cannot_prove" {
  value = {
    dynamodb = "streams, TTL firing, adaptive capacity, real transaction conflict, PITR, IAM"
    minio    = "bucket-policy denial, s3:signatureAge, SSE-KMS and bucket keys, AWS checksum semantics, lifecycle"
    postgres = "Aurora ACU scaling and cold resume, failover, the Data API transport, IAM auth, PITR"
  }
  description = "What each substitute cannot prove. Every entry maps to a seam with `requires_live = true`; a green local run is never evidence for these."
}
