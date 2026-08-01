output "repository_url" {
  value       = aws_ecr_repository.this.repository_url
  description = "Repository URL. Images are always referenced as `<url>@sha256:<digest>`, never `<url>:<tag>`."
}

output "arn" {
  value       = aws_ecr_repository.this.arn
  description = "ARN of the repository."
}
