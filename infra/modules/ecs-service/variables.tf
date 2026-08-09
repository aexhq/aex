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
  description = "Number of tasks. `brain-mux` runs at least two in the production plane and exactly one in development. Session state is not held by the task: the journal, the lease/fence and the work and wake state are the authority, so a second task takes work it can serve. Stable task affinity is an accelerator over that authority, never a correctness condition."

  validation {
    condition     = var.desired_count >= 1 && var.desired_count <= 100
    error_message = "The desired count must be between 1 and 100."
  }

  validation {
    condition     = var.name != "brain-mux" || !startswith(var.task_definition_family, "aex-prd-") || var.desired_count >= 2
    error_message = "Production `brain-mux` must run at least two tasks. A single task is one point of loss for a workload whose state authority is already durable."
  }

  validation {
    condition     = var.name != "brain-mux" || !startswith(var.task_definition_family, "aex-dev-") || var.desired_count == 1
    error_message = "Development `brain-mux` runs exactly one task."
  }
}

variable "stop_timeout" {
  type        = number
  description = "Seconds a container is given to drain before it is killed. 120 for `brain-mux`, 30 for `regional-stream` and `regional-session-api`."

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

  validation {
    condition     = var.name != "regional-session-api" || var.stop_timeout == 30
    error_message = "`regional-session-api` must use a 30 second stop timeout. It is the drain deadline the process's own `AEX_SESSION_DRAIN_DEADLINE_MS` is set below, so the task finishes its in-flight requests and exits on its own terms rather than being killed mid-transaction."
  }
}

# The gap that turns a slow start into a failed apply. ECS begins counting load
# balancer health-check failures the moment a task reaches RUNNING; with
# `wait_for_steady_state` and the circuit breaker both on, a service that needs
# longer than one unhealthy window to answer its first probe does not deploy
# slowly, it fails the apply and rolls back.
variable "health_check_grace_period_seconds" {
  type        = number
  default     = null
  description = "Seconds ECS ignores load balancer health checks after a task starts. Required for a service behind a load balancer; meaningless, and rejected by ECS, without one."

  validation {
    condition = (
      var.health_check_grace_period_seconds == null
      || (var.health_check_grace_period_seconds >= 0 && var.health_check_grace_period_seconds <= 2147483647)
    )
    error_message = "The health check grace period must be between 0 and 2147483647 seconds."
  }

  validation {
    condition     = var.target_group_arn == null || var.health_check_grace_period_seconds != null
    error_message = "A service registered with a load balancer must state its health check grace period. Without one ECS counts health-check failures from the first second, and `wait_for_steady_state` plus the deployment circuit breaker turn a slow first start into a failed apply rather than a slow one."
  }

  validation {
    condition     = var.target_group_arn != null || var.health_check_grace_period_seconds == null
    error_message = "A grace period only applies to a service behind a load balancer; ECS rejects it on a service with no load balancer."
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

    # An undimensioned metric in an AWS-owned namespace does not describe this
    # service, it describes every load balancer or cluster in the account
    # aggregated together. The policy still applies, still looks healthy, and
    # scales on somebody else's traffic.
    dimensions = optional(map(string), {})

    # Asymmetric on purpose: come up quickly when the signal says saturated,
    # retreat slowly so a brief dip does not shed capacity a moment before the
    # next burst needs it.
    scale_out_cooldown = optional(number, 60)
    scale_in_cooldown  = optional(number, 300)
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

  validation {
    condition = alltrue([
      for m in var.autoscaling_metrics :
      length(m.dimensions) > 0
      if startswith(m.namespace, "AWS/") || startswith(m.namespace, "ECS/")
    ])
    error_message = "A metric in an AWS-owned namespace such as `AWS/ApplicationELB` or `ECS/ContainerInsights` must carry dimensions. Undimensioned, CloudWatch aggregates every load balancer or cluster in the account into one series, and the policy scales this service on somebody else's traffic while looking perfectly healthy."
  }

  validation {
    condition = alltrue([
      for m in var.autoscaling_metrics :
      alltrue([for k, v in m.dimensions : length(k) > 0 && length(v) > 0])
    ])
    error_message = "Every metric dimension must have a non-empty name and value."
  }

  validation {
    condition = alltrue([
      for m in var.autoscaling_metrics :
      m.scale_out_cooldown >= 0 && m.scale_out_cooldown <= 3600
      && m.scale_in_cooldown >= 0 && m.scale_in_cooldown <= 3600
    ])
    error_message = "Both cooldowns must be between 0 and 3600 seconds."
  }

  validation {
    condition = alltrue([
      for m in var.autoscaling_metrics : m.scale_in_cooldown >= m.scale_out_cooldown
    ])
    error_message = "The scale-in cooldown must be at least the scale-out cooldown. Shedding capacity faster than it is added is how a service oscillates under a load pattern it should simply have absorbed."
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
    condition = var.name != "brain-mux" || (
      var.autoscaling_bounds.min_capacity == var.desired_count
      && var.autoscaling_bounds.max_capacity == var.desired_count
    )
    error_message = "`brain-mux` runs a static task floor for the first alpha, so both capacity bounds must equal its desired count. Concurrency is raised on measured evidence, never by a scaling policy."
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
  description = "Environment variables. Every key is namespaced. `brain-mux` must also carry `AEX_MAX_ACTIVE_ACTIVATIONS`, the approved launch profile: the binary requires that variable and supplies no default, so a task definition that omits it deploys a container that refuses to start."

  validation {
    condition     = alltrue([for k in keys(var.env) : can(regex("^AEX_[A-Z0-9_]+$", k))])
    error_message = "Every environment variable key must match `^AEX_[A-Z0-9_]+$`."
  }

  validation {
    condition     = var.name != "brain-mux" || lookup(var.env, "AEX_MAX_ACTIVE_ACTIVATIONS", "") == "16"
    error_message = "`brain-mux` must be given the approved launch profile `AEX_MAX_ACTIVE_ACTIVATIONS = \"16\"`. Sixteen active activations per task is a decision to observe before raising concurrency rather than a capacity finding, and the binary has no default to fall back to: an omitted value is a task that never starts, and a different one is a plane running bands nobody approved."
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

variable "vpc_id" {
  type        = string
  description = "VPC the tasks' own security group is created in. It must be the VPC the subnets above belong to; a group is only meaningful inside one."

  validation {
    condition     = can(regex("^vpc-[0-9a-f]{8,17}$", var.vpc_id))
    error_message = "The VPC must be an EC2 VPC id such as `vpc-0123456789abcdef0`."
  }
}

variable "interface_endpoint_security_group_id" {
  type        = string
  description = "The group every private AWS interface endpoint in this VPC shares, as `vpc-regional` exports it. The tasks are given TLS egress to it, which is how they reach ECR, CloudWatch Logs, KMS, Secrets Manager, STS and SQS with no NAT gateway and no route to the internet."

  validation {
    condition     = can(regex("^sg-[0-9a-f]{8,17}$", var.interface_endpoint_security_group_id))
    error_message = "The interface endpoint security group must be an EC2 security group id such as `sg-0123456789abcdef0`."
  }
}

variable "gateway_endpoint_prefix_list_ids" {
  type        = map(string)
  description = "Gateway endpoint service short name to AWS-managed prefix-list id, as `vpc-regional` exports it. A gateway endpoint is a route table entry rather than an interface, so it has no security group to reference and its prefix list is the only way an egress rule can name it."

  validation {
    condition     = length(setsubtract(["s3", "dynamodb"], keys(var.gateway_endpoint_prefix_list_ids))) == 0
    error_message = "The map must carry both `s3` and `dynamodb`. S3 is where the image layers the task starts from are read, and DynamoDB is the journal; a task that cannot reach either does not run, and the failure surfaces as a timeout rather than as a denial."
  }

  validation {
    condition     = alltrue([for id in values(var.gateway_endpoint_prefix_list_ids) : can(regex("^pl-[0-9a-f]{8,17}$", id))])
    error_message = "Every gateway endpoint value must be an AWS-managed prefix-list id such as `pl-0123456789abcdef0`."
  }
}

# A list of at most one rather than a single id, because the *number* of rules
# has to be known while planning and the id itself does not. The load balancer's
# group is created in the same plan as the service that sits behind it, so its
# id is unknown until apply; a `count` written over `id == null` cannot be
# planned at all, and the root fails with "the count value depends on resource
# attributes that cannot be determined until apply". The length of a
# one-element list is known even when the element is not.
variable "load_balancer_security_group_ids" {
  type        = list(string)
  default     = []
  description = "The security group of the load balancer in front of this service, as a list of at most one, or empty for a service with no edge. It is the only source the tasks admit, and this module also opens the matching egress on it: the load balancer's own module cannot name a task group without depending on the service that already depends on it, and only the service knows which port to open."

  validation {
    condition     = length(var.load_balancer_security_group_ids) <= 1
    error_message = "A service sits behind at most one load balancer. This is a list only so the rule count is known while planning; it is not a way to admit several edges."
  }

  validation {
    condition     = alltrue([for id in var.load_balancer_security_group_ids : can(regex("^sg-[0-9a-f]{8,17}$", id))])
    error_message = "The load balancer security group must be an EC2 security group id such as `sg-0123456789abcdef0`."
  }

  validation {
    condition     = var.target_group_arn == null || length(var.load_balancer_security_group_ids) > 0
    error_message = "A service registered with a target group must name the load balancer's security group. Without it the tasks admit nothing, the load balancer has no egress towards them, and every target reads unhealthy while the service, the target group and the listener all look correctly configured."
  }

  validation {
    condition     = var.target_group_arn != null || length(var.load_balancer_security_group_ids) == 0
    error_message = "A load balancer security group only means something for a service registered with a target group. Naming one without a target group would admit an edge that never sends this service traffic."
  }
}

variable "additional_security_group_ids" {
  type        = list(string)
  default     = []
  description = "Groups attached to the tasks alongside the one this module creates. The module's own group is always attached; anything here is added to it and never substituted for it, so no caller can leave the tasks reachable through a group nothing in the repository builds."

  validation {
    condition     = alltrue([for id in var.additional_security_group_ids : can(regex("^sg-[0-9a-f]{8,17}$", id))])
    error_message = "Every additional security group must be an EC2 security group id such as `sg-0123456789abcdef0`."
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
