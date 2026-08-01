output "table_names" {
  value       = local.physical_names
  description = "Logical table name to physical table name. This is the map an environment binding records."
}

output "table_arns" {
  value       = { for k, t in aws_dynamodb_table.this : k => t.arn }
  description = "Logical table name to table ARN."
}

output "stream_arns" {
  value = {
    for k, t in aws_dynamodb_table.this : k => t.stream_arn
    if local.by_name[k].stream_view_type != null
  }
  description = "Logical table name to stream ARN, for the tables that enable a stream."
}
