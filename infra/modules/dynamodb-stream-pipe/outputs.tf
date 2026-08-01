output "pipe_arn" {
  value       = aws_pipes_pipe.this.arn
  description = "ARN of the pipe."
}

output "pipe_role_arn" {
  value       = aws_iam_role.pipe.arn
  description = "ARN of the role the pipe assumes. It is the only writer this module grants on the target queue."
}
