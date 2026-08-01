resource "aws_scheduler_schedule_group" "this" {
  name = var.group_name
}

resource "aws_scheduler_schedule" "this" {
  for_each = { for s in var.schedules : s.name => s }

  name                         = each.value.name
  description                  = each.value.description
  group_name                   = aws_scheduler_schedule_group.this.name
  schedule_expression          = each.value.expression
  schedule_expression_timezone = each.value.expression_timezone
  kms_key_arn                  = var.kms_key_arn
  state                        = "ENABLED"

  flexible_time_window {
    mode                      = each.value.flexible_window_minutes == 0 ? "OFF" : "FLEXIBLE"
    maximum_window_in_minutes = each.value.flexible_window_minutes == 0 ? null : each.value.flexible_window_minutes
  }

  target {
    arn      = each.value.target_arn
    role_arn = each.value.role_arn
    input    = each.value.input

    retry_policy {
      maximum_retry_attempts       = each.value.maximum_retry_attempts
      maximum_event_age_in_seconds = each.value.maximum_event_age_in_sec
    }

    dynamic "dead_letter_config" {
      for_each = each.value.dead_letter_arn == null ? [] : [each.value.dead_letter_arn]

      content {
        arn = dead_letter_config.value
      }
    }
  }
}
