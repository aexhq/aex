variable "plane" {
  type        = string
  description = "Deployment plane."
}

variable "region" {
  type        = string
  description = "Region the central plane runs in."
}

variable "permissions_boundary_policy_arn" {
  type        = string
  description = "Owner-managed permissions boundary required on every deployable execution role."
}

variable "subnet_ids" {
  type        = list(string)
  description = "Private subnets from the region foundation."
}

variable "security_group_ids" {
  type        = list(string)
  description = "Security groups for the cluster and the database."
}

variable "kms_key_arn" {
  type        = string
  description = "Customer-managed key for the queue, the database and the log group."
}

# --- the public edge ----------------------------------------------------------
#
# None of the five below existed in this root before `central-api`. The central
# plane had no load balancer, no cluster and no public subnets at all: every
# central deployable was a Lambda behind an API Gateway, and a gateway needs
# none of them.

variable "vpc_id" {
  type        = string
  description = "The VPC the load balancer and the central service run in."
}

variable "public_subnet_ids" {
  type        = list(string)
  description = "Public subnets for the load balancer. The service itself stays in `subnet_ids`."
}

variable "cluster_name" {
  type        = string
  description = "The ECS cluster the central service runs on."
}

variable "interface_endpoint_security_group_id" {
  type        = string
  description = "The interface endpoint group the task reaches AWS APIs through. There is no NAT path: the task has no public address and every AWS call leaves through an endpoint."
}

variable "gateway_endpoint_prefix_list_ids" {
  type        = map(string)
  description = "Gateway endpoint prefix lists, keyed `s3` and `dynamodb`, for the task's egress rules."
}

variable "alb" {
  type = object({
    name               = string
    certificate_arn    = string
    access_logs_bucket = string
  })
  description = "The central plane's public load balancer. A second standing hourly cost the plane did not carry before; the alternative — a second target group on the regional load balancer — needs host-header conditions `alb-service-target` cannot express."
}

variable "central_api" {
  type = object({
    name                              = string
    image                             = string
    cpu                               = number
    memory                            = number
    desired_count                     = number
    stop_timeout                      = number
    container_port                    = number
    health_check_grace_period_seconds = number
    log_group_name                    = string
    log_retention_days                = number
    execution_role_arn                = string
    env                               = map(string)
    rules = list(object({
      priority      = number
      path_patterns = list(string)
    }))
    autoscaling_bounds = object({
      min_capacity = number
      max_capacity = number
    })
    autoscaling_metrics = list(object({
      name               = string
      namespace          = string
      statistic          = string
      target_value       = number
      dimensions         = optional(map(string), {})
      scale_out_cooldown = optional(number, 60)
      scale_in_cooldown  = optional(number, 300)
    }))
  })
  description = "The merged central HTTP service. It replaces `central-control-api`, `central-identity-api` and `finance-api` as the plane's public surface."

  validation {
    condition     = length(var.central_api.rules) > 0
    error_message = "At least one listener rule is required; a service with no rule receives nothing and the load balancer answers its default 404 for the whole central plane."
  }

  validation {
    condition     = length(distinct([for r in var.central_api.rules : r.priority])) == length(var.central_api.rules)
    error_message = "Two rules share a priority. A listener rejects a duplicate priority at apply; catching it here keeps the collision out of the plane."
  }

  validation {
    condition     = length(distinct(flatten([for r in var.central_api.rules : r.path_patterns]))) == length(flatten([for r in var.central_api.rules : r.path_patterns]))
    error_message = "A path pattern is repeated across rules. Two rules matching one request means evaluation order decides which forwards it, and that ordering is not something anybody reviewed."
  }

  validation {
    condition     = var.central_api.stop_timeout == 30
    error_message = "`central-api` must use a 30 second stop timeout. The process derives its admitted `AEX_CENTRAL_API_DRAIN_DEADLINE_MS` ceiling from this exact number (`aex_regional_http::drain::FARGATE_STOP_TIMEOUT_S`) and refuses to start on a deadline that could not fire before SIGKILL. Changing it here without changing that constant makes the drain deadline unreachable and the task is killed mid-transaction instead of exiting on its own terms."
  }
}

variable "device_flow_rate_limit" {
  type = object({
    name       = string
    rate_limit = number
    paths      = list(string)
  })
  description = <<-EOT
    The per-source-IP throttle on the unauthenticated device-flow routes.

    API Gateway supplied the only request throttle the central plane ever had,
    and an Application Load Balancer has none. These two routes are the ones with
    nothing else in front of them: they admit no credential by design, and every
    anonymous caller shares one replay principal. Every other central route now
    resolves its credential against Aurora on each request, so an unthrottled
    flood there is also a load amplifier onto the control database — but those
    callers are at least identified, which is why the limit is scoped down rather
    than applied plane-wide.
  EOT

  validation {
    condition     = length(var.device_flow_rate_limit.paths) > 0
    error_message = "The throttle must name at least one path. An empty list means the web ACL is billed monthly and matches nothing."
  }
}

variable "artifact_bucket" {
  type        = string
  description = "Bucket the publish lane wrote the Lambda ZIP to."
}

variable "finance_api" {
  type = object({
    function_name           = string
    artifact_key            = string
    artifact_object_version = string
    artifact_sha256         = string
    memory_mb               = number
    timeout_s               = number
    log_retention_days      = number
    env                     = map(string)
  })
  description = "The finance API Lambda. The artifact was built, digested and published long before this plan runs."
}

variable "settlement_queue" {
  type = object({
    name                      = string
    visibility_timeout        = number
    max_receive_count         = number
    message_retention_seconds = number
    dlq_retention_seconds     = number
    batch_size                = number
  })
  description = "The settlement work queue and its consumer batching."
}

variable "database" {
  type = object({
    cluster_identifier        = string
    database_name             = string
    master_username           = string
    engine_version            = string
    min_acu                   = number
    max_acu                   = number
    backup_retention_days     = number
    final_snapshot_identifier = string
  })
  description = "The central finance cluster, including the final snapshot identity required by its recoverable delete path."
}

variable "schema_admin" {
  type = object({
    family             = string
    image              = string
    cpu                = number
    memory             = number
    stop_timeout       = number
    log_group_name     = string
    secret_env         = map(string)
    execution_role_arn = string
  })
  description = "The one-shot schema admin task. The image is digest-pinned and is itself the central migration-bytes identity."
}

variable "deployable_grants" {
  type = map(object({
    assume_principal = object({
      type        = string
      identifiers = list(string)
    })
    wildcard_resource_allowlist = list(string)
    action_grants = list(object({
      sid                = string
      actions            = list(string)
      resources          = list(string)
      scopable           = bool
      condition_operator = optional(string, "StringEquals")
      condition_key      = optional(string)
      condition_values   = optional(list(string))
    }))
  }))
  description = "Execution role grants per deployable, taken from the generated IAM action lists."
}

variable "schedules" {
  type = list(object({
    name                    = string
    description             = string
    expression              = string
    flexible_window_minutes = number
    target_kind             = string
    input                   = string
    role_arn                = string
  }))
  description = "Scheduled work. `target_kind` selects which deployable this root points the schedule at, so the root never has to name a mutable target."
}

variable "schedule_group_name" {
  type        = string
  description = "Schedule group name."
}

variable "tags" {
  type        = map(string)
  description = "Tags applied to everything in the central application."
  default     = {}
}
