resource "aws_lambda_event_source_mapping" "this" {
  event_source_arn = var.source_arn
  function_name    = var.function_alias_arn
  enabled          = var.enabled

  batch_size                         = var.batch_size
  maximum_batching_window_in_seconds = var.max_batching_window
  starting_position                  = var.starting_position

  function_response_types = var.partial_batch_response ? ["ReportBatchItemFailures"] : []

  dynamic "scaling_config" {
    for_each = var.scaling_config == null ? [] : [var.scaling_config]

    content {
      maximum_concurrency = scaling_config.value.maximum_concurrency
    }
  }

  tags = var.tags
}
