locals {
  autoscaling_enabled = length(var.autoscaling_metrics) > 0
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

resource "aws_ecs_service" "this" {
  name            = var.name
  cluster         = var.cluster_arn
  task_definition = aws_ecs_task_definition.this.arn
  desired_count   = var.desired_count
  launch_type     = "FARGATE"
  propagate_tags  = "SERVICE"
  tags            = var.tags

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
    security_groups  = var.security_group_ids
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

  lifecycle {
    ignore_changes = [desired_count]
  }
}

resource "aws_appautoscaling_target" "this" {
  count = local.autoscaling_enabled ? 1 : 0

  service_namespace  = "ecs"
  scalable_dimension = "ecs:service:DesiredCount"
  resource_id        = "service/${var.cluster_name}/${var.name}"
  min_capacity       = var.autoscaling_bounds.min_capacity
  max_capacity       = var.autoscaling_bounds.max_capacity
}

resource "aws_appautoscaling_policy" "this" {
  for_each = local.autoscaling_enabled ? { for m in var.autoscaling_metrics : m.name => m } : {}

  name               = "${var.name}-${lower(each.key)}"
  policy_type        = "TargetTrackingScaling"
  service_namespace  = aws_appautoscaling_target.this[0].service_namespace
  scalable_dimension = aws_appautoscaling_target.this[0].scalable_dimension
  resource_id        = aws_appautoscaling_target.this[0].resource_id

  target_tracking_scaling_policy_configuration {
    target_value = each.value.target_value

    customized_metric_specification {
      metric_name = each.value.name
      namespace   = each.value.namespace
      statistic   = each.value.statistic
    }
  }
}
