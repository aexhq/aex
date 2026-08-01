locals {
  by_name = { for t in var.table_definitions : t.logical_name => t }

  physical_names = {
    for t in var.table_definitions : t.logical_name => (
      t.logical_name == "keystore"
      ? var.keystore_physical_name
      : "${var.name_prefix}${replace(t.logical_name, "_", "-")}"
    )
  }
}

resource "aws_dynamodb_table" "this" {
  for_each = local.by_name

  name         = local.physical_names[each.key]
  billing_mode = each.value.billing_mode
  hash_key     = each.value.hash_key
  range_key    = each.value.range_key

  deletion_protection_enabled = each.value.deletion_protection
  stream_enabled              = each.value.stream_view_type != null
  stream_view_type            = each.value.stream_view_type

  dynamic "attribute" {
    for_each = each.value.attributes

    content {
      name = attribute.value.name
      type = attribute.value.type
    }
  }

  dynamic "global_secondary_index" {
    for_each = coalesce(each.value.global_secondary_indexes, [])

    content {
      name               = global_secondary_index.value.name
      hash_key           = global_secondary_index.value.hash_key
      range_key          = global_secondary_index.value.range_key
      projection_type    = global_secondary_index.value.projection_type
      non_key_attributes = global_secondary_index.value.non_key_attributes
    }
  }

  dynamic "ttl" {
    for_each = each.value.ttl_attribute == null ? [] : [each.value.ttl_attribute]

    content {
      enabled        = true
      attribute_name = ttl.value
    }
  }

  point_in_time_recovery {
    enabled                 = true
    recovery_period_in_days = each.value.point_in_time_recovery_days
  }

  server_side_encryption {
    enabled     = true
    kms_key_arn = var.kms_key_arn_by_authority[each.value.authority]
  }

  tags = merge(var.tags, {
    "aex:plane"     = var.plane
    "aex:region"    = var.region
    "aex:authority" = each.value.authority
  })

  lifecycle {
    precondition {
      condition     = each.key != "keystore" || local.physical_names[each.key] == var.keystore_physical_name
      error_message = "The keystore table must be created under its pinned physical name, not under the environment name prefix."
    }
  }
}
