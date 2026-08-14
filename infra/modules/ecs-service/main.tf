locals {
  autoscaling_enabled = length(var.autoscaling_metrics) > 0
  security_group_ids  = concat([aws_security_group.task.id], var.additional_security_group_ids)
}

# The tasks' group is the service's to own. While no module created it and no
# environment root was allowed to declare one, it could not exist anywhere, and
# every root that wired `security_group_ids` was naming a group nothing in the
# repository builds.
#
# The name is derived from the plane- and region-qualified family rather than
# from `name`, because `name` is bare - `session-api` - and both planes live
# in one account. Group names are unique per VPC, so the bare name would work
# and would still read as though it named the only one.
resource "aws_security_group" "task" {
  name        = "${var.task_definition_family}-task"
  description = "Egress from the ${var.name} tasks to the private AWS endpoints they need"
  vpc_id      = var.vpc_id

  tags = merge(var.tags, { Name = "${var.task_definition_family}-task" })
}

# Exactly one source, and the health check arrives on the same port as the
# traffic, so there is no second rule to write.
resource "aws_vpc_security_group_ingress_rule" "from_load_balancer" {
  count = length(var.load_balancer_security_group_ids)

  security_group_id            = aws_security_group.task.id
  description                  = "Requests and health checks from the load balancer in front of this service"
  ip_protocol                  = "tcp"
  from_port                    = var.container_port
  to_port                      = var.container_port
  referenced_security_group_id = var.load_balancer_security_group_ids[count.index]
}

# The other half of that edge, written here because this is the only place both
# ends are in scope. `alb-public` creates the group but cannot reference a task
# group without depending on the service that depends on it, and one load
# balancer carries several services, so the port is per service rather than per
# load balancer. Terraform revokes the allow-all egress AWS attaches to a new
# group, so without this rule the load balancer reaches nothing.
resource "aws_vpc_security_group_egress_rule" "load_balancer_to_task" {
  count = length(var.load_balancer_security_group_ids)

  security_group_id            = var.load_balancer_security_group_ids[count.index]
  description                  = "To the ${var.name} tasks on the port they listen on"
  ip_protocol                  = "tcp"
  from_port                    = var.container_port
  to_port                      = var.container_port
  referenced_security_group_id = aws_security_group.task.id
}

# The direct-client edge, for a service reached by name rather than through a
# load balancer. Written here for the same reason the load balancer's egress rule
# is: this is the only place both ends are in scope, and only the service knows
# which port to open.
resource "aws_vpc_security_group_ingress_rule" "from_client" {
  count = length(var.client_security_group_ids)

  security_group_id            = aws_security_group.task.id
  description                  = "Requests from a service allowed to call this one directly"
  ip_protocol                  = "tcp"
  from_port                    = var.container_port
  to_port                      = var.container_port
  referenced_security_group_id = var.client_security_group_ids[count.index]
}

resource "aws_vpc_security_group_egress_rule" "client_to_task" {
  count = length(var.client_security_group_ids)

  security_group_id            = var.client_security_group_ids[count.index]
  description                  = "To the ${var.name} tasks on the port they listen on"
  ip_protocol                  = "tcp"
  from_port                    = var.container_port
  to_port                      = var.container_port
  referenced_security_group_id = aws_security_group.task.id
}

# Egress is these three rules and nothing else - no `0.0.0.0/0`. The VPC has no
# NAT gateway by default, so anything not named here is unreachable rather than
# merely unauthorised.
#
# Task role credentials are not a fourth rule: they arrive over the link-local
# task metadata address, which is served by the instance the task runs on and is
# not filtered by any security group.
resource "aws_vpc_security_group_egress_rule" "interface_endpoints" {
  security_group_id            = aws_security_group.task.id
  description                  = "TLS to the private AWS interface endpoints. They all share one group, so this single rule covers every endpoint the VPC carries."
  ip_protocol                  = "tcp"
  from_port                    = 443
  to_port                      = 443
  referenced_security_group_id = var.interface_endpoint_security_group_id
}

resource "aws_vpc_security_group_egress_rule" "gateway_endpoints" {
  for_each = var.gateway_endpoint_prefix_list_ids

  security_group_id = aws_security_group.task.id
  description       = "TLS to the ${each.key} gateway endpoint, named by its AWS-managed prefix list because a gateway endpoint is a route rather than an interface and has no group to reference"
  ip_protocol       = "tcp"
  from_port         = 443
  to_port           = 443
  prefix_list_id    = each.value
}

# Provider and remote-MCP endpoints do not have stable AWS prefix lists. The
# network boundary therefore opens only TCP/443, while Brain/Tool Mux retain
# the hostname, redirect, DNS and credential policy. Authority APIs leave this
# disabled and cannot reach the public internet at all.
resource "aws_vpc_security_group_egress_rule" "public_https" {
  count = var.public_https_egress ? 1 : 0

  security_group_id = aws_security_group.task.id
  description       = "HTTPS to validated public provider or MCP endpoints"
  ip_protocol       = "tcp"
  from_port         = 443
  to_port           = 443
  cidr_ipv4         = "0.0.0.0/0"
}

resource "aws_ecs_task_definition" "this" {
  family                   = var.task_definition_family
  skip_destroy             = true
  cpu                      = tostring(var.cpu)
  memory                   = tostring(var.memory)
  network_mode             = "awsvpc"
  requires_compatibilities = ["FARGATE"]
  task_role_arn            = var.task_role_arn
  execution_role_arn       = var.execution_role_arn
  tags                     = var.tags

  runtime_platform {
    cpu_architecture        = var.runtime_platform.cpu_architecture
    operating_system_family = var.runtime_platform.operating_system_family
  }

  container_definitions = jsonencode([
    {
      name        = var.name
      image       = var.image
      essential   = true
      stopTimeout = var.stop_timeout

      portMappings = [
        { containerPort = var.container_port, protocol = "tcp" },
      ]

      environment = [
        for k, v in var.env : { name = k, value = v }
      ]

      secrets = [
        for k, v in var.secret_env : { name = k, valueFrom = v }
      ]

      logConfiguration = {
        logDriver = "awslogs"
        options = {
          "awslogs-group"         = var.log_group_name
          "awslogs-region"        = var.region
          "awslogs-stream-prefix" = var.name
        }
      }
    },
  ])
}

# One service, two declarations, because a lifecycle block cannot be
# conditional. `ignore_changes = [desired_count]` is correct exactly when an
# autoscaling target owns the count; applied to a fixed-count service it turned
# a reviewed desired_count raise into a green no-op apply that changed nothing.
# The `count` guards keep exactly one of the two in any configuration.

resource "aws_ecs_service" "autoscaled" {
  count = local.autoscaling_enabled ? 1 : 0

  name            = var.name
  cluster         = var.cluster_arn
  task_definition = aws_ecs_task_definition.this.arn
  desired_count   = var.desired_count
  launch_type     = "FARGATE"
  propagate_tags  = "SERVICE"
  tags            = var.tags

  force_new_deployment = var.deployment_trigger != null
  triggers = var.deployment_trigger == null ? {} : {
    deployment_attempt = var.deployment_trigger
  }

  health_check_grace_period_seconds = var.health_check_grace_period_seconds

  # A first deployment whose tasks crash otherwise exits terraform 0 with the
  # service parked at zero tasks; waiting for steady state fails the apply loudly.
  wait_for_steady_state = true

  timeouts {
    create = "15m"
    update = "15m"
  }

  deployment_circuit_breaker {
    enable   = var.circuit_breaker.enable
    rollback = var.circuit_breaker.rollback
  }

  network_configuration {
    subnets          = var.subnets
    security_groups  = local.security_group_ids
    assign_public_ip = false
  }

  dynamic "load_balancer" {
    for_each = var.target_group_arn == null ? [] : [var.target_group_arn]

    content {
      target_group_arn = load_balancer.value
      container_name   = var.name
      container_port   = var.container_port
    }
  }

  dynamic "service_registries" {
    for_each = var.service_discovery_arn == null ? [] : [var.service_discovery_arn]

    content {
      registry_arn = service_registries.value
    }
  }

  lifecycle {
    # The autoscaling target owns the live count; terraform re-imposing
    # `desired_count` on every apply would fight it.
    ignore_changes = [desired_count]
  }
}

resource "aws_ecs_service" "static" {
  count = local.autoscaling_enabled ? 0 : 1

  name            = var.name
  cluster         = var.cluster_arn
  task_definition = aws_ecs_task_definition.this.arn
  desired_count   = var.desired_count
  launch_type     = "FARGATE"
  propagate_tags  = "SERVICE"
  tags            = var.tags

  force_new_deployment = var.deployment_trigger != null
  triggers = var.deployment_trigger == null ? {} : {
    deployment_attempt = var.deployment_trigger
  }

  health_check_grace_period_seconds = var.health_check_grace_period_seconds

  # A first deployment whose tasks crash otherwise exits terraform 0 with the
  # service parked at zero tasks; waiting for steady state fails the apply loudly.
  wait_for_steady_state = true

  timeouts {
    create = "15m"
    update = "15m"
  }

  deployment_circuit_breaker {
    enable   = var.circuit_breaker.enable
    rollback = var.circuit_breaker.rollback
  }

  network_configuration {
    subnets          = var.subnets
    security_groups  = local.security_group_ids
    assign_public_ip = false
  }

  dynamic "load_balancer" {
    for_each = var.target_group_arn == null ? [] : [var.target_group_arn]

    content {
      target_group_arn = load_balancer.value
      container_name   = var.name
      container_port   = var.container_port
    }
  }

  dynamic "service_registries" {
    for_each = var.service_discovery_arn == null ? [] : [var.service_discovery_arn]

    content {
      registry_arn = service_registries.value
    }
  }

  # Deliberately no lifecycle block: with no autoscaling target, terraform is
  # the only writer of desired_count, and a raised count must apply.
}

resource "aws_appautoscaling_target" "this" {
  count = local.autoscaling_enabled ? 1 : 0

  service_namespace  = "ecs"
  scalable_dimension = "ecs:service:DesiredCount"
  resource_id        = "service/${var.cluster_name}/${var.name}"
  min_capacity       = var.autoscaling_bounds.min_capacity
  max_capacity       = var.autoscaling_bounds.max_capacity

  # The resource id is a derived string, so terraform sees no edge to the
  # service; without this the target can be registered before the service
  # exists and the apply fails on ordering rather than on substance.
  depends_on = [aws_ecs_service.autoscaled]
}

resource "aws_appautoscaling_policy" "this" {
  for_each = local.autoscaling_enabled ? { for m in var.autoscaling_metrics : m.name => m } : {}

  name               = "${var.name}-${lower(each.key)}"
  policy_type        = "TargetTrackingScaling"
  service_namespace  = aws_appautoscaling_target.this[0].service_namespace
  scalable_dimension = aws_appautoscaling_target.this[0].scalable_dimension
  resource_id        = aws_appautoscaling_target.this[0].resource_id

  target_tracking_scaling_policy_configuration {
    target_value       = each.value.target_value
    scale_out_cooldown = each.value.scale_out_cooldown
    scale_in_cooldown  = each.value.scale_in_cooldown

    customized_metric_specification {
      metric_name = each.value.name
      namespace   = each.value.namespace
      statistic   = each.value.statistic

      # Without these an AWS-owned namespace resolves to every load balancer or
      # cluster in the account aggregated together, and the policy scales this
      # service on traffic that is not its own.
      dynamic "dimensions" {
        for_each = each.value.dimensions

        content {
          name  = dimensions.key
          value = dimensions.value
        }
      }
    }
  }
}
