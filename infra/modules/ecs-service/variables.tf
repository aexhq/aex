variable "name" {
  type        = string
  description = "Service name. It also selects the pins that apply to `brain-mux` and `regional-stream`."

  validation {
    condition     = can(regex("^[a-z][a-z0-9-]{2,50}$", var.name))
    error_message = "The service name must be lowercase and hyphen-separated."
  }
}

variable "task_definition_family" {
  type        = string
  description = "Plane- and region-qualified ECS task definition family for the service."

  validation {
    condition = (
      can(regex("^aex-(dev|prd)-[a-z]{2}-[a-z]+-[0-9]-[a-z][a-z0-9-]{2,50}$", var.task_definition_family))
      && endswith(var.task_definition_family, "-${var.name}")
    )
    error_message = "The task definition family must be `aex-<dev|prd>-<region>-<service-name>`."
  }
}

variable "cluster_arn" {
  type        = string
  description = "ECS cluster the service runs in."

  validation {
    condition     = can(regex("^arn:aws[a-z-]*:ecs:[a-z0-9-]+:[0-9A-Za-z-]{1,64}:cluster/[A-Za-z0-9_-]+$", var.cluster_arn))
    error_message = "The cluster must be an ECS cluster ARN."
  }
}

variable "cluster_name" {
  type        = string
  description = "ECS cluster name, needed to address the autoscaling target."

  validation {
    condition     = can(regex("^[A-Za-z0-9_-]{1,255}$", var.cluster_name))
    error_message = "The cluster name must be a valid ECS cluster name."
  }
}

variable "image" {
  type        = string
  description = "Container image, pinned by digest. A tag is rejected: a service that follows a tag can restart onto different bytes with no deployment and no receipt."

  validation {
    condition     = can(regex("^[^:]+@sha256:[0-9a-f]{64}$", var.image))
    error_message = "The image must be digest-pinned as `<repository>@sha256:<64 hex>`; a `:tag` reference is not accepted."
  }
}

variable "cpu" {
  type        = number
  description = "Task CPU units."

  validation {
    condition     = contains([256, 512, 1024, 2048, 4096, 8192, 16384], var.cpu)
    error_message = "The CPU value must be a valid Fargate task size."
  }
}

variable "memory" {
  type        = number
  description = "Task memory in megabytes."

  validation {
    condition     = var.memory >= 512 && var.memory <= 122880
    error_message = "Memory must be between 512 and 122880 megabytes."
  }
}

variable "runtime_platform" {
  type = object({
    cpu_architecture        = string
    operating_system_family = string
  })
  default = {
    cpu_architecture        = "ARM64"
    operating_system_family = "LINUX"
  }
  description = "Explicit runtime platform, so an OCI index cannot silently select a different child."

  validation {
    condition     = contains(["ARM64", "X86_64"], var.runtime_platform.cpu_architecture)
    error_message = "The CPU architecture must be `ARM64` or `X86_64`."
  }
}

variable "desired_count" {
  type        = number
  default     = 1
  description = "Number of tasks. `brain-mux` is pinned to 1 until the multi-task session-affinity decision lands: a second task would take sessions it holds no state for."

  validation {
    condition     = var.desired_count >= 1 && var.desired_count <= 100
    error_message = "The desired count must be between 1 and 100."
  }

  validation {
    condition     = var.name != "brain-mux" || var.desired_count == 1
    error_message = "`brain-mux` is pinned to a single task until the multi-task session-affinity decision lands."
  }
}

variable "stop_timeout" {
  type        = number
  description = "Seconds a container is given to drain before it is killed. 120 for `brain-mux`, 30 for `regional-stream`."

  validation {
    condition     = var.stop_timeout >= 1 && var.stop_timeout <= 120
    error_message = "The stop timeout must be between 1 and 120 seconds."
  }

  validation {
    condition     = var.name != "brain-mux" || var.stop_timeout == 120
    error_message = "`brain-mux` must be given the full 120 seconds to drain an in-flight turn."
  }

  validation {
    condition     = var.name != "regional-stream" || var.stop_timeout == 30
    error_message = "`regional-stream` must use a 30 second stop timeout."
  }
}

variable "deregistration_delay" {
  type        = number
  default     = 30
  description = "Seconds the load balancer keeps draining a deregistering target."

  validation {
    condition     = var.deregistration_delay >= 30 && var.deregistration_delay <= 3600
    error_message = "The deregistration delay must be at least 30 seconds."
  }
}

variable "circuit_breaker" {
  type = object({
    enable   = bool
    rollback = bool
  })
  default = {
    enable   = true
    rollback = true
  }
  description = "Deployment circuit breaker. It stays enabled: without it a deployment that never becomes healthy just keeps trying."

  validation {
    condition     = var.circuit_breaker.enable
    error_message = "The deployment circuit breaker must be enabled."
  }
}

variable "autoscaling_metrics" {
  type = list(object({
    name         = string
    namespace    = string
    statistic    = string
    target_value = number
  }))
  description = "Target-tracking metrics. An empty list selects fixed-count mode. When metrics are supplied, at least one must be service-published: CPU alone does not describe a queueing workload, so scaling on it alone hides saturation."

  validation {
    condition     = length(var.autoscaling_metrics) == 0 || anytrue([for m in var.autoscaling_metrics : m.namespace != "AWS/ECS"])
    error_message = "At least one autoscaling metric must be a custom, service-published metric; CPU alone is not an acceptable scaling signal."
  }

  validation {
    condition     = alltrue([for m in var.autoscaling_metrics : contains(["Average", "Maximum", "Minimum", "SampleCount", "Sum"], m.statistic)])
    error_message = "Every metric statistic must be a CloudWatch statistic."
  }

  validation {
    condition     = alltrue([for m in var.autoscaling_metrics : m.target_value > 0])
    error_message = "Every metric target value must be positive."
  }
}

variable "autoscaling_bounds" {
  type = object({
    min_capacity = number
    max_capacity = number
  })
  default = {
    min_capacity = 1
    max_capacity = 1
  }
  description = "Capacity bounds. Without autoscaling metrics, both values must equal `desired_count`."

  validation {
    condition     = var.autoscaling_bounds.min_capacity >= 1 && var.autoscaling_bounds.max_capacity >= var.autoscaling_bounds.min_capacity
    error_message = "The maximum capacity must be at least the minimum, and the minimum at least 1."
  }

  validation {
    condition     = var.name != "brain-mux" || var.autoscaling_bounds.max_capacity == 1
    error_message = "`brain-mux` is pinned to a single task, so its autoscaling ceiling must also be 1."
  }

  validation {
    condition = (
      length(var.autoscaling_metrics) > 0
      || (
        var.autoscaling_bounds.min_capacity == var.desired_count
        && var.autoscaling_bounds.max_capacity == var.desired_count
      )
    )
    error_message = "Without autoscaling metrics, minimum and maximum capacity must both equal desired_count."
  }
}

variable "env" {
  type        = map(string)
  default     = {}
  description = "Environment variables. Every key is namespaced."

  validation {
    condition     = alltrue([for k in keys(var.env) : can(regex("^AEX_[A-Z0-9_]+$", k))])
    error_message = "Every environment variable key must match `^AEX_[A-Z0-9_]+$`."
  }
}

variable "secret_env" {
  type        = map(string)
  default     = {}
  description = "Environment variables sourced by ARN reference."

  validation {
    condition     = alltrue([for v in values(var.secret_env) : startswith(v, "arn:")])
    error_message = "Every secret value must be an ARN reference; a plaintext secret is not accepted."
  }
}

variable "container_port" {
  type        = number
  description = "Port the container listens on."

  validation {
    condition     = var.container_port >= 1 && var.container_port <= 65535
    error_message = "The container port must be a valid port number."
  }
}

variable "target_group_arn" {
  type        = string
  default     = null
  description = "Target group to register with, when the service sits behind a load balancer."

  validation {
    condition     = var.target_group_arn == null || can(regex("^arn:aws[a-z-]*:elasticloadbalancing:", coalesce(var.target_group_arn, "none")))
    error_message = "When set, the target group must be an ELB target group ARN."
  }
}

variable "task_role_arn" {
  type        = string
  description = "Task role the container runs as."

  validation {
    condition     = can(regex("^arn:aws[a-z-]*:iam::[0-9A-Za-z-]{1,64}:role/[A-Za-z0-9+=,.@_/-]+$", var.task_role_arn))
    error_message = "The task role must be an IAM role ARN."
  }
}

variable "execution_role_arn" {
  type        = string
  description = "Execution role the ECS agent uses to pull the image and write logs."

  validation {
    condition     = can(regex("^arn:aws[a-z-]*:iam::[0-9A-Za-z-]{1,64}:role/[A-Za-z0-9+=,.@_/-]+$", var.execution_role_arn))
    error_message = "The execution role must be an IAM role ARN."
  }
}

variable "subnets" {
  type        = list(string)
  description = "Private subnets the tasks run in."

  validation {
    condition     = length(var.subnets) > 0
    error_message = "At least one subnet is required."
  }
}

variable "security_group_ids" {
  type        = list(string)
  description = "Security groups attached to the tasks."

  validation {
    condition     = length(var.security_group_ids) > 0
    error_message = "At least one security group is required."
  }
}

variable "log_group_name" {
  type        = string
  description = "CloudWatch log group the container writes to."

  validation {
    condition     = startswith(var.log_group_name, "/aex/")
    error_message = "The log group must live under `/aex/`."
  }
}

variable "region" {
  type        = string
  description = "AWS region, used for the log driver configuration."

  validation {
    condition     = can(regex("^[a-z]{2}-[a-z]+-[0-9]$", var.region))
    error_message = "The region must be an AWS region code such as `eu-west-1`."
  }
}

variable "tags" {
  type        = map(string)
  default     = {}
  description = "Tags applied to the service and the task definition."
}
