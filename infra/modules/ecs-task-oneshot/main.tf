# A one-shot task is a task definition and nothing else. There is deliberately
# no aws_ecs_service here: a service would keep restarting a job that is meant
# to run once, succeed or fail, and stop.
resource "aws_ecs_task_definition" "this" {
  family                   = var.family
  skip_destroy             = true
  cpu                      = tostring(var.cpu)
  memory                   = tostring(var.memory)
  network_mode             = "awsvpc"
  requires_compatibilities = ["FARGATE"]
  task_role_arn            = var.role_arn
  execution_role_arn       = var.execution_role_arn
  tags                     = var.tags

  runtime_platform {
    cpu_architecture        = var.runtime_platform.cpu_architecture
    operating_system_family = var.runtime_platform.operating_system_family
  }

  container_definitions = jsonencode([
    {
      name        = var.family
      image       = var.image
      essential   = true
      stopTimeout = var.stop_timeout

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
          "awslogs-stream-prefix" = var.family
        }
      }
    },
  ])
}
