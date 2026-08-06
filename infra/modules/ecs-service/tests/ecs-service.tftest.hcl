mock_provider "aws" {}

variables {
  name                   = "regional-stream"
  task_definition_family = "aex-dev-eu-west-1-regional-stream"
  cluster_arn            = "arn:aws:ecs:eu-west-1:000000000000:cluster/aex-dev-euw1"
  cluster_name           = "aex-dev-euw1"
  image                  = "000000000000.dkr.ecr.eu-west-1.amazonaws.com/aex/regional-stream@sha256:0000000000000000000000000000000000000000000000000000000000000000"
  cpu                    = 1024
  memory                 = 2048
  stop_timeout           = 30
  container_port         = 8080
  task_role_arn          = "arn:aws:iam::000000000000:role/aex-dev-regional-stream"
  execution_role_arn     = "arn:aws:iam::000000000000:role/aex-dev-ecs-execution"
  subnets                = ["subnet-0123456789abcdef0"]
  security_group_ids     = ["sg-0123456789abcdef0"]
  log_group_name         = "/aex/dev/regional-stream"
  region                 = "eu-west-1"

  autoscaling_bounds = {
    min_capacity = 1
    max_capacity = 6
  }

  autoscaling_metrics = [
    {
      name         = "StreamBacklogSeconds"
      namespace    = "AEX/RegionalStream"
      statistic    = "Average"
      target_value = 5
    },
  ]
}

run "task_definition_identity_is_explicit_and_retained" {
  command = plan

  assert {
    condition     = aws_ecs_task_definition.this.family == var.task_definition_family
    error_message = "The task definition family must use the caller's plane-qualified identity, independently of the service name."
  }

  assert {
    condition     = aws_ecs_task_definition.this.skip_destroy == true
    error_message = "Terraform must retain old task definition revisions so the release role does not need unscopable deregistration authority."
  }
}

run "the_image_is_digest_pinned" {
  command = plan

  assert {
    condition     = jsondecode(aws_ecs_task_definition.this.container_definitions)[0].image == var.image
    error_message = "The service must run the digest-pinned image it was given."
  }

  assert {
    condition     = can(regex("@sha256:[0-9a-f]{64}$", jsondecode(aws_ecs_task_definition.this.container_definitions)[0].image))
    error_message = "The container image must be digest-pinned."
  }
}

run "the_circuit_breaker_is_enabled_and_rolls_back" {
  command = plan

  assert {
    condition     = one(aws_ecs_service.this.deployment_circuit_breaker).enable == true
    error_message = "The deployment circuit breaker must be enabled."
  }

  assert {
    condition     = one(aws_ecs_service.this.deployment_circuit_breaker).rollback == true
    error_message = "A tripped circuit breaker must roll back."
  }
}

run "autoscaling_targets_a_service_published_metric" {
  command = plan

  assert {
    condition = anytrue([
      for k, p in aws_appautoscaling_policy.this :
      one(one(p.target_tracking_scaling_policy_configuration).customized_metric_specification).namespace != "AWS/ECS"
    ])
    error_message = "At least one scaling policy must track a service-published metric, not CPU alone."
  }

  assert {
    condition = alltrue([
      for k, p in aws_appautoscaling_policy.this : p.policy_type == "TargetTrackingScaling"
    ])
    error_message = "Every scaling policy must be target tracking."
  }
}

run "tasks_never_get_a_public_address" {
  command = plan

  assert {
    condition     = one(aws_ecs_service.this.network_configuration).assign_public_ip == false
    error_message = "Service tasks must not be given a public address."
  }
}

run "regional_stream_drains_for_thirty_seconds" {
  command = plan

  assert {
    condition     = jsondecode(aws_ecs_task_definition.this.container_definitions)[0].stopTimeout == 30
    error_message = "regional-stream must use a 30 second stop timeout."
  }
}

run "brain_mux_is_pinned_to_one_task_and_drains_for_two_minutes" {
  command = plan

  variables {
    name                   = "brain-mux"
    task_definition_family = "aex-dev-eu-west-1-brain-mux"
    stop_timeout           = 120
    log_group_name         = "/aex/dev/brain-mux"

    autoscaling_bounds = {
      min_capacity = 1
      max_capacity = 1
    }
  }

  assert {
    condition     = aws_ecs_service.this.desired_count == 1
    error_message = "brain-mux must run exactly one task."
  }

  assert {
    condition     = jsondecode(aws_ecs_task_definition.this.container_definitions)[0].stopTimeout == 120
    error_message = "brain-mux must be given the full 120 seconds to drain an in-flight turn."
  }

  assert {
    condition     = one(aws_appautoscaling_target.this).max_capacity == 1
    error_message = "brain-mux must not be able to scale past a single task."
  }
}

run "rejects_a_tag_reference" {
  command = plan

  variables {
    image = "000000000000.dkr.ecr.eu-west-1.amazonaws.com/aex/regional-stream:v1"
  }

  expect_failures = [var.image]
}

run "rejects_a_second_brain_mux_task" {
  command = plan

  variables {
    name                   = "brain-mux"
    task_definition_family = "aex-dev-eu-west-1-brain-mux"
    stop_timeout           = 120
    desired_count          = 2

    autoscaling_bounds = {
      min_capacity = 1
      max_capacity = 1
    }
  }

  expect_failures = [var.desired_count]
}

run "rejects_a_brain_mux_stop_timeout_that_is_not_two_minutes" {
  command = plan

  variables {
    name                   = "brain-mux"
    task_definition_family = "aex-dev-eu-west-1-brain-mux"
    stop_timeout           = 30

    autoscaling_bounds = {
      min_capacity = 1
      max_capacity = 1
    }
  }

  expect_failures = [var.stop_timeout]
}

run "rejects_an_unqualified_task_definition_family" {
  command = plan

  variables {
    task_definition_family = "regional-stream"
  }

  expect_failures = [var.task_definition_family]
}

run "rejects_a_regional_stream_stop_timeout_that_is_not_thirty_seconds" {
  command = plan

  variables {
    stop_timeout = 120
  }

  expect_failures = [var.stop_timeout]
}

run "rejects_disabling_the_circuit_breaker" {
  command = plan

  variables {
    circuit_breaker = {
      enable   = false
      rollback = false
    }
  }

  expect_failures = [var.circuit_breaker]
}

run "rejects_scaling_on_cpu_alone" {
  command = plan

  variables {
    autoscaling_metrics = [
      {
        name         = "CPUUtilization"
        namespace    = "AWS/ECS"
        statistic    = "Average"
        target_value = 60
      },
    ]
  }

  expect_failures = [var.autoscaling_metrics]
}

run "rejects_a_deregistration_delay_below_thirty_seconds" {
  command = plan

  variables {
    deregistration_delay = 5
  }

  expect_failures = [var.deregistration_delay]
}

run "rejects_an_environment_key_outside_the_aex_namespace" {
  command = plan

  variables {
    env = {
      RUST_LOG = "info"
    }
  }

  expect_failures = [var.env]
}

run "fixed_count_creates_no_autoscaling_resources" {
  command = plan

  variables {
    desired_count = 2

    autoscaling_bounds = {
      min_capacity = 2
      max_capacity = 2
    }

    autoscaling_metrics = []
  }

  assert {
    condition     = aws_ecs_service.this.desired_count == 2
    error_message = "Fixed-count mode must preserve the reviewed desired task count."
  }

  assert {
    condition     = length(aws_appautoscaling_target.this) == 0
    error_message = "Fixed-count mode must not create an Application Auto Scaling target."
  }

  assert {
    condition     = length(aws_appautoscaling_policy.this) == 0
    error_message = "Fixed-count mode must not create Application Auto Scaling policies."
  }
}

run "rejects_uncollapsed_bounds_without_autoscaling_metrics" {
  command = plan

  variables {
    desired_count = 2

    autoscaling_bounds = {
      min_capacity = 1
      max_capacity = 2
    }

    autoscaling_metrics = []
  }

  expect_failures = [var.autoscaling_bounds]
}
