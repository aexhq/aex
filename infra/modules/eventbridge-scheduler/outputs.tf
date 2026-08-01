output "schedule_arns" {
  value       = { for k, s in aws_scheduler_schedule.this : k => s.arn }
  description = "Schedule name to schedule ARN."
}

output "group_name" {
  value       = aws_scheduler_schedule_group.this.name
  description = "Name of the schedule group."
}
