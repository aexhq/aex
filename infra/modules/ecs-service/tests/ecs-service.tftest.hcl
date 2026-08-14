mock_provider "aws" {}

# The task group this module creates has no id until it exists, and the network
# configuration it lands in is asserted below, so the plan needs one.
override_resource {
  target          = aws_security_group.task
  override_during = plan
  values = {
    id = "sg-0123456789abcdef0"
  }
}

variables {
  name                   = "session-api"
  task_definition_family = "aex-dev-eu-west-1-session-api"
  cluster_arn            = "arn:aws:ecs:eu-west-1:000000000000:cluster/aex-dev-euw1"
  cluster_name           = "aex-dev-euw1"
  image                  = "000000000000.dkr.ecr.eu-west-1.amazonaws.com/aex/session-api@sha256:0000000000000000000000000000000000000000000000000000000000000000"
  cpu                    = 1024
  memory                 = 2048
  stop_timeout           = 30
  container_port         = 8080
  task_role_arn          = "arn:aws:iam::000000000000:role/aex-dev-session-api"
  execution_role_arn     = "arn:aws:iam::000000000000:role/aex-dev-ecs-execution"
  subnets                = ["subnet-0123456789abcdef0"]
  log_group_name         = "/aex/dev/session-api"
  region                 = "eu-west-1"

  vpc_id                               = "vpc-0123456789abcdef0"
  interface_endpoint_security_group_id = "sg-0123456789abcdef1"

  gateway_endpoint_prefix_list_ids = {
    s3       = "pl-0123456789abcdef0"
    dynamodb = "pl-0123456789abcdef1"
  }

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

run "the_circuit_breaker_is_enabled_without_automatic_rollback" {
  command = plan

  assert {
    condition     = one(aws_ecs_service.autoscaled[0].deployment_circuit_breaker).enable == true
    error_message = "The deployment circuit breaker must be enabled."
  }

  assert {
    condition     = one(aws_ecs_service.autoscaled[0].deployment_circuit_breaker).rollback == false
    error_message = "A tripped circuit breaker must stop the deployment for a fix-forward release, not attempt an automatic rollback."
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

run "the_apply_waits_for_steady_state" {
  command = plan

  assert {
    condition     = aws_ecs_service.autoscaled[0].wait_for_steady_state == true
    error_message = "The apply must wait for the service to reach steady state; without it a first deployment whose tasks crash exits successfully with the service parked at zero tasks."
  }

  assert {
    condition     = length(aws_ecs_service.static) == 0
    error_message = "An autoscaled configuration must materialize only the desired-count-ignoring variant."
  }
}

run "tasks_never_get_a_public_address" {
  command = plan

  assert {
    condition     = one(aws_ecs_service.autoscaled[0].network_configuration).assign_public_ip == false
    error_message = "Service tasks must not be given a public address."
  }
}

run "session_api_drains_for_thirty_seconds" {
  command = plan

  assert {
    condition     = jsondecode(aws_ecs_task_definition.this.container_definitions)[0].stopTimeout == 30
    error_message = "session-api must use a 30 second stop timeout."
  }
}

run "development_brain_mux_runs_one_task_and_drains_for_two_minutes" {
  command = plan

  variables {
    name                   = "brain-mux"
    task_definition_family = "aex-dev-eu-west-1-brain-mux"
    stop_timeout           = 120
    log_group_name         = "/aex/dev/brain-mux"
    env                    = { AEX_MAX_ACTIVE_ACTIVATIONS = "16" }

    autoscaling_bounds = {
      min_capacity = 1
      max_capacity = 1
    }
  }

  assert {
    condition     = aws_ecs_service.autoscaled[0].desired_count == 1
    error_message = "Development brain-mux runs exactly one task."
  }

  assert {
    condition     = jsondecode(aws_ecs_task_definition.this.container_definitions)[0].stopTimeout == 120
    error_message = "brain-mux must be given the full 120 seconds to drain an in-flight turn."
  }

  assert {
    condition     = one(aws_appautoscaling_target.this).max_capacity == 1
    error_message = "brain-mux must not be able to scale past its declared task floor."
  }

  assert {
    condition = length([
      for entry in jsondecode(aws_ecs_task_definition.this.container_definitions)[0].environment :
      entry if entry.name == "AEX_MAX_ACTIVE_ACTIVATIONS" && entry.value == "16"
    ]) == 1
    error_message = "The approved 16-activation launch profile must reach the container exactly once. The variable has no default in the binary, so an absent one is a task that refuses to start."
  }
}

run "production_brain_mux_runs_the_approved_two_task_floor" {
  command = plan

  variables {
    name                   = "brain-mux"
    task_definition_family = "aex-prd-eu-west-1-brain-mux"
    stop_timeout           = 120
    log_group_name         = "/aex/prd/brain-mux"
    desired_count          = 2
    env                    = { AEX_MAX_ACTIVE_ACTIVATIONS = "16" }

    autoscaling_bounds = {
      min_capacity = 2
      max_capacity = 2
    }
  }

  assert {
    condition     = aws_ecs_service.autoscaled[0].desired_count == 2
    error_message = "Production brain-mux must run the approved two-task floor."
  }

  assert {
    condition = length([
      for entry in jsondecode(aws_ecs_task_definition.this.container_definitions)[0].environment :
      entry if entry.name == "AEX_MAX_ACTIVE_ACTIVATIONS" && entry.value == "16"
    ]) == 1
    error_message = "The launch profile is the same 16 activations per task in both planes; the second production task is a placement decision, not a bigger budget."
  }

  assert {
    condition     = one(aws_appautoscaling_target.this).max_capacity == 2
    error_message = "The production brain-mux floor is static: its ceiling is its desired count."
  }
}

run "rejects_a_single_production_brain_mux_task" {
  command = plan

  variables {
    name                   = "brain-mux"
    task_definition_family = "aex-prd-eu-west-1-brain-mux"
    stop_timeout           = 120
    log_group_name         = "/aex/prd/brain-mux"
    desired_count          = 1
    env                    = { AEX_MAX_ACTIVE_ACTIVATIONS = "16" }

    autoscaling_bounds = {
      min_capacity = 1
      max_capacity = 1
    }
  }

  expect_failures = [var.desired_count]
}

run "rejects_brain_mux_capacity_bounds_that_could_scale" {
  command = plan

  variables {
    name                   = "brain-mux"
    task_definition_family = "aex-prd-eu-west-1-brain-mux"
    stop_timeout           = 120
    log_group_name         = "/aex/prd/brain-mux"
    desired_count          = 2
    env                    = { AEX_MAX_ACTIVE_ACTIVATIONS = "16" }

    autoscaling_bounds = {
      min_capacity = 2
      max_capacity = 4
    }
  }

  expect_failures = [var.autoscaling_bounds]
}

run "rejects_a_brain_mux_without_the_approved_activation_budget" {
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

  expect_failures = [var.env]
}

run "rejects_a_brain_mux_activation_budget_that_is_not_the_approved_profile" {
  command = plan

  variables {
    name                   = "brain-mux"
    task_definition_family = "aex-prd-eu-west-1-brain-mux"
    stop_timeout           = 120
    log_group_name         = "/aex/prd/brain-mux"
    desired_count          = 2
    env                    = { AEX_MAX_ACTIVE_ACTIVATIONS = "48" }

    autoscaling_bounds = {
      min_capacity = 2
      max_capacity = 2
    }
  }

  expect_failures = [var.env]
}

run "rejects_a_tag_reference" {
  command = plan

  variables {
    image = "000000000000.dkr.ecr.eu-west-1.amazonaws.com/aex/session-api:v1"
  }

  expect_failures = [var.image]
}

run "rejects_a_second_development_brain_mux_task" {
  command = plan

  variables {
    name                   = "brain-mux"
    task_definition_family = "aex-dev-eu-west-1-brain-mux"
    stop_timeout           = 120
    desired_count          = 2
    env                    = { AEX_MAX_ACTIVE_ACTIVATIONS = "16" }

    autoscaling_bounds = {
      min_capacity = 2
      max_capacity = 2
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
    env                    = { AEX_MAX_ACTIVE_ACTIVATIONS = "16" }

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
    task_definition_family = "session-api"
  }

  expect_failures = [var.task_definition_family]
}

run "rejects_a_session_api_stop_timeout_that_is_not_thirty_seconds" {
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

run "rejects_enabling_automatic_rollback" {
  command = plan

  variables {
    circuit_breaker = {
      enable   = true
      rollback = true
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
    condition     = aws_ecs_service.static[0].desired_count == 2
    error_message = "Fixed-count mode must preserve the reviewed desired task count."
  }

  assert {
    condition     = length(aws_ecs_service.autoscaled) == 0 && length(aws_ecs_service.static) == 1
    error_message = "Fixed-count mode must materialize exactly the variant whose desired_count terraform enforces; the desired-count-ignoring variant would turn a raised count into a green no-op."
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

# --- the request path -------------------------------------------------------

run "a_load_balanced_service_gets_a_health_check_grace_period" {
  command = plan

  variables {
    target_group_arn                  = "arn:aws:elasticloadbalancing:eu-west-1:000000000000:targetgroup/aex-dev-euw1-stream-tg/73e2d6bc24d8a067"
    load_balancer_security_group_ids  = ["sg-0123456789abcdef2"]
    health_check_grace_period_seconds = 60
  }

  assert {
    condition     = aws_ecs_service.autoscaled[0].health_check_grace_period_seconds == 60
    error_message = "The grace period must reach the service. Without it ECS counts load balancer health-check failures from the first second, and wait_for_steady_state plus the circuit breaker turn a slow first start into a failed apply."
  }

  assert {
    condition     = one(aws_ecs_service.autoscaled[0].load_balancer).container_port == var.container_port
    error_message = "The load balancer registration must name the port the container actually listens on."
  }
}

run "a_fixed_count_load_balanced_service_gets_the_same_grace_period" {
  command = plan

  variables {
    desired_count                     = 2
    target_group_arn                  = "arn:aws:elasticloadbalancing:eu-west-1:000000000000:targetgroup/aex-dev-euw1-stream-tg/73e2d6bc24d8a067"
    load_balancer_security_group_ids  = ["sg-0123456789abcdef2"]
    health_check_grace_period_seconds = 90

    autoscaling_bounds = {
      min_capacity = 2
      max_capacity = 2
    }

    autoscaling_metrics = []
  }

  assert {
    condition     = aws_ecs_service.static[0].health_check_grace_period_seconds == 90
    error_message = "Both service variants must honour the grace period; a fixed-count request-path service starts no faster than an autoscaled one."
  }
}

run "rejects_a_load_balanced_service_with_no_health_check_grace_period" {
  command = plan

  variables {
    target_group_arn                 = "arn:aws:elasticloadbalancing:eu-west-1:000000000000:targetgroup/aex-dev-euw1-stream-tg/73e2d6bc24d8a067"
    load_balancer_security_group_ids = ["sg-0123456789abcdef2"]
  }

  expect_failures = [var.health_check_grace_period_seconds]
}

run "rejects_a_grace_period_on_a_service_with_no_load_balancer" {
  command = plan

  variables {
    health_check_grace_period_seconds = 60
  }

  expect_failures = [var.health_check_grace_period_seconds]
}

# --- scaling signals describe this service, not the account -----------------

run "metric_dimensions_reach_the_scaling_policy" {
  command = plan

  variables {
    autoscaling_metrics = [
      {
        name         = "RequestCountPerTarget"
        namespace    = "AWS/ApplicationELB"
        statistic    = "Sum"
        target_value = 400
        dimensions = {
          TargetGroup = "targetgroup/aex-dev-euw1-stream-tg/73e2d6bc24d8a067"
        }
      },
      {
        name         = "StreamBacklogSeconds"
        namespace    = "AEX/RegionalStream"
        statistic    = "Average"
        target_value = 5
      },
    ]
  }

  assert {
    condition = alltrue([
      for d in one(one(aws_appautoscaling_policy.this["RequestCountPerTarget"].target_tracking_scaling_policy_configuration).customized_metric_specification).dimensions :
      d.name == "TargetGroup" && d.value == "targetgroup/aex-dev-euw1-stream-tg/73e2d6bc24d8a067"
    ])
    error_message = "The declared dimensions must reach the metric specification; undimensioned, an AWS/ApplicationELB metric is every load balancer in the account aggregated together."
  }

  assert {
    condition     = length(one(one(aws_appautoscaling_policy.this["RequestCountPerTarget"].target_tracking_scaling_policy_configuration).customized_metric_specification).dimensions) == 1
    error_message = "Exactly the declared dimensions must be emitted."
  }

  assert {
    condition     = length(one(one(aws_appautoscaling_policy.this["StreamBacklogSeconds"].target_tracking_scaling_policy_configuration).customized_metric_specification).dimensions) == 0
    error_message = "A service-published metric that declares no dimensions must emit none."
  }
}

run "rejects_an_undimensioned_metric_in_an_aws_owned_namespace" {
  command = plan

  variables {
    autoscaling_metrics = [
      {
        name         = "RequestCountPerTarget"
        namespace    = "AWS/ApplicationELB"
        statistic    = "Sum"
        target_value = 400
      },
    ]
  }

  expect_failures = [var.autoscaling_metrics]
}

run "rejects_an_undimensioned_container_insights_metric" {
  command = plan

  variables {
    autoscaling_metrics = [
      {
        name         = "TaskCount"
        namespace    = "ECS/ContainerInsights"
        statistic    = "Average"
        target_value = 10
      },
    ]
  }

  expect_failures = [var.autoscaling_metrics]
}

run "the_scaling_cooldowns_reach_the_policy" {
  command = plan

  variables {
    autoscaling_metrics = [
      {
        name               = "StreamBacklogSeconds"
        namespace          = "AEX/RegionalStream"
        statistic          = "Average"
        target_value       = 5
        scale_out_cooldown = 30
        scale_in_cooldown  = 600
      },
    ]
  }

  assert {
    condition     = one(aws_appautoscaling_policy.this["StreamBacklogSeconds"].target_tracking_scaling_policy_configuration).scale_out_cooldown == 30
    error_message = "The declared scale-out cooldown must reach the policy."
  }

  assert {
    condition     = one(aws_appautoscaling_policy.this["StreamBacklogSeconds"].target_tracking_scaling_policy_configuration).scale_in_cooldown == 600
    error_message = "The declared scale-in cooldown must reach the policy."
  }
}

run "the_default_cooldowns_retreat_more_slowly_than_they_advance" {
  command = plan

  assert {
    condition     = one(aws_appautoscaling_policy.this["StreamBacklogSeconds"].target_tracking_scaling_policy_configuration).scale_out_cooldown == 60
    error_message = "The default scale-out cooldown must be 60 seconds."
  }

  assert {
    condition     = one(aws_appautoscaling_policy.this["StreamBacklogSeconds"].target_tracking_scaling_policy_configuration).scale_in_cooldown == 300
    error_message = "The default scale-in cooldown must be 300 seconds, so a brief dip does not shed capacity the next burst needs."
  }
}

run "rejects_shedding_capacity_faster_than_it_is_added" {
  command = plan

  variables {
    autoscaling_metrics = [
      {
        name               = "StreamBacklogSeconds"
        namespace          = "AEX/RegionalStream"
        statistic          = "Average"
        target_value       = 5
        scale_out_cooldown = 300
        scale_in_cooldown  = 60
      },
    ]
  }

  expect_failures = [var.autoscaling_metrics]
}

# --- session-api -------------------------------------------------------------
#
# The suite default above already carries this name, so the 30-second
# stop-timeout pin is exercised by
# `rejects_a_session_api_stop_timeout_that_is_not_thirty_seconds`.
# What is left here is the shape a public-edge service must carry.

run "session_api_drains_for_thirty_seconds_behind_a_target_group" {
  command = plan

  variables {
    stop_timeout  = 30
    desired_count = 2

    target_group_arn                  = "arn:aws:elasticloadbalancing:eu-west-1:000000000000:targetgroup/aex-dev-euw1-edge-tg/73e2d6bc24d8a067"
    load_balancer_security_group_ids  = ["sg-0123456789abcdef2"]
    health_check_grace_period_seconds = 60

    autoscaling_bounds = {
      min_capacity = 2
      max_capacity = 2
    }

    autoscaling_metrics = []
  }

  assert {
    condition     = jsondecode(aws_ecs_task_definition.this.container_definitions)[0].stopTimeout == 30
    error_message = "session-api must use a 30 second stop timeout; the process derives its own admitted drain deadline ceiling from it."
  }

  assert {
    condition     = aws_ecs_service.static[0].desired_count == 2
    error_message = "The reviewed session-api floor is two tasks."
  }

  assert {
    condition     = aws_ecs_service.static[0].health_check_grace_period_seconds == 60
    error_message = "A request-path service behind the public load balancer must carry a grace period."
  }
}

run "rejects_a_session_api_stop_timeout_below_the_pin" {
  command = plan

  variables {
    stop_timeout = 10
  }

  expect_failures = [var.stop_timeout]
}

# --- the tasks' own security group ------------------------------------------

run "the_service_creates_the_group_its_tasks_run_with" {
  command = plan

  assert {
    condition     = aws_security_group.task.vpc_id == var.vpc_id
    error_message = "The task security group must be created by this module, in the VPC its subnets belong to. No module created these groups and no environment root may declare one, so a group nobody creates is a service that cannot be applied at all."
  }

  assert {
    condition     = aws_security_group.task.name == "${var.task_definition_family}-task"
    error_message = "The group name must be derived from the plane- and region-qualified family, never supplied. The bare service name would read as though it named the only `session-api` group, and both planes live in one account."
  }

  assert {
    condition     = contains(one(aws_ecs_service.autoscaled[0].network_configuration).security_groups, aws_security_group.task.id)
    error_message = "The tasks must run with the group this module creates."
  }

  assert {
    condition     = length(one(aws_ecs_service.autoscaled[0].network_configuration).security_groups) == 1
    error_message = "With no additional groups supplied, the module's own group must be the only one the tasks run with."
  }
}

run "task_egress_reaches_the_private_endpoints_and_nothing_else" {
  command = plan

  assert {
    condition = (
      aws_vpc_security_group_egress_rule.interface_endpoints.ip_protocol == "tcp"
      && aws_vpc_security_group_egress_rule.interface_endpoints.from_port == 443
      && aws_vpc_security_group_egress_rule.interface_endpoints.to_port == 443
      && aws_vpc_security_group_egress_rule.interface_endpoints.referenced_security_group_id == var.interface_endpoint_security_group_id
    )
    error_message = "One rule to the shared interface endpoint group is how the tasks reach ECR, CloudWatch Logs, KMS, Secrets Manager, STS and SQS. Every interface endpoint sits in that one group, so one rule covers all of them."
  }

  assert {
    condition = alltrue([
      for k, rule in aws_vpc_security_group_egress_rule.gateway_endpoints :
      rule.ip_protocol == "tcp" && rule.from_port == 443 && rule.to_port == 443
      && rule.prefix_list_id == var.gateway_endpoint_prefix_list_ids[k]
    ])
    error_message = "Each gateway endpoint must be named by its AWS-managed prefix list. A gateway endpoint is a route table entry rather than an interface, so it has no security group a rule could reference."
  }

  assert {
    condition     = length(aws_vpc_security_group_egress_rule.gateway_endpoints) == 2
    error_message = "Exactly the two gateway endpoints - S3 for the image layers, DynamoDB for the journal - must be reachable."
  }

  assert {
    condition = alltrue(concat(
      [aws_vpc_security_group_egress_rule.interface_endpoints.cidr_ipv4 == null],
      [for rule in aws_vpc_security_group_egress_rule.gateway_endpoints : rule.cidr_ipv4 == null],
    ))
    error_message = "No task egress rule may name a CIDR. The VPC has no NAT gateway, so an open egress rule would not grant reach it would merely stop describing what the tasks may do; the endpoint group and the two prefix lists are the whole of it."
  }
}

# Task role credentials arrive over the link-local metadata address, which no
# security group filters, so there is no fourth egress rule to look for.
run "a_service_with_no_load_balancer_admits_nothing_at_all" {
  command = plan

  assert {
    condition     = length(aws_vpc_security_group_ingress_rule.from_load_balancer) == 0
    error_message = "A service with no load balancer in front of it must admit no ingress whatsoever."
  }

  assert {
    condition     = length(aws_vpc_security_group_egress_rule.load_balancer_to_task) == 0
    error_message = "With no load balancer there is no group to open towards the tasks."
  }
}

run "a_load_balanced_service_admits_only_that_load_balancer" {
  command = plan

  variables {
    target_group_arn                  = "arn:aws:elasticloadbalancing:eu-west-1:000000000000:targetgroup/aex-dev-euw1-stream-tg/73e2d6bc24d8a067"
    load_balancer_security_group_ids  = ["sg-0123456789abcdef2"]
    health_check_grace_period_seconds = 60
  }

  assert {
    condition     = length(aws_vpc_security_group_ingress_rule.from_load_balancer) == 1
    error_message = "The load balancer is the one source the tasks admit. The health check arrives on the same port as the traffic, so it needs no rule of its own."
  }

  assert {
    condition = (
      one(aws_vpc_security_group_ingress_rule.from_load_balancer).referenced_security_group_id == one(var.load_balancer_security_group_ids)
      && one(aws_vpc_security_group_ingress_rule.from_load_balancer).cidr_ipv4 == null
    )
    error_message = "Ingress must name the load balancer's group rather than any address range, so a subnet CIDR cannot quietly widen into a second source."
  }

  assert {
    condition = (
      one(aws_vpc_security_group_ingress_rule.from_load_balancer).from_port == var.container_port
      && one(aws_vpc_security_group_ingress_rule.from_load_balancer).to_port == var.container_port
    )
    error_message = "The admitted port must be the port the container listens on, not a range around it."
  }
}

run "the_load_balancer_is_opened_towards_this_service_and_its_port" {
  command = plan

  variables {
    target_group_arn                  = "arn:aws:elasticloadbalancing:eu-west-1:000000000000:targetgroup/aex-dev-euw1-stream-tg/73e2d6bc24d8a067"
    load_balancer_security_group_ids  = ["sg-0123456789abcdef2"]
    health_check_grace_period_seconds = 60
  }

  assert {
    condition     = one(aws_vpc_security_group_egress_rule.load_balancer_to_task).security_group_id == one(var.load_balancer_security_group_ids)
    error_message = "The matching egress must land on the load balancer's group. Terraform revokes the allow-all egress AWS attaches to a new group, so without this rule the edge reaches nothing and every target reads unhealthy while the service, the target group and the listener all look correct."
  }

  assert {
    condition = (
      one(aws_vpc_security_group_egress_rule.load_balancer_to_task).referenced_security_group_id == aws_security_group.task.id
      && one(aws_vpc_security_group_egress_rule.load_balancer_to_task).from_port == var.container_port
    )
    error_message = "The load balancer must be opened towards this service's own task group on this service's own port. One load balancer carries several services, which is why the rule belongs to the service rather than to the edge."
  }
}

run "additional_groups_are_added_to_the_module_group_not_substituted_for_it" {
  command = plan

  variables {
    additional_security_group_ids = ["sg-0123456789abcdef3"]
  }

  assert {
    condition     = contains(one(aws_ecs_service.autoscaled[0].network_configuration).security_groups, aws_security_group.task.id)
    error_message = "The module's own group must stay attached whatever else a caller supplies; a caller may add reach a task needs but may not replace what this module states about it."
  }

  assert {
    condition     = length(one(aws_ecs_service.autoscaled[0].network_configuration).security_groups) == 2
    error_message = "An additional group must be attached alongside the module's own, not instead of it."
  }
}

run "rejects_a_vpc_id_that_is_not_a_vpc" {
  command = plan

  variables {
    vpc_id = "subnet-0123456789abcdef0"
  }

  expect_failures = [var.vpc_id]
}

run "rejects_an_interface_endpoint_group_that_is_not_a_security_group" {
  command = plan

  variables {
    interface_endpoint_security_group_id = "pl-0123456789abcdef0"
  }

  expect_failures = [var.interface_endpoint_security_group_id]
}

run "rejects_a_gateway_endpoint_map_missing_the_journal" {
  command = plan

  variables {
    gateway_endpoint_prefix_list_ids = {
      s3 = "pl-0123456789abcdef0"
    }
  }

  expect_failures = [var.gateway_endpoint_prefix_list_ids]
}

run "rejects_a_gateway_endpoint_named_by_anything_but_a_prefix_list" {
  command = plan

  variables {
    gateway_endpoint_prefix_list_ids = {
      s3       = "sg-0123456789abcdef0"
      dynamodb = "pl-0123456789abcdef1"
    }
  }

  expect_failures = [var.gateway_endpoint_prefix_list_ids]
}

run "rejects_a_load_balancer_group_that_is_not_a_security_group" {
  command = plan

  variables {
    target_group_arn                  = "arn:aws:elasticloadbalancing:eu-west-1:000000000000:targetgroup/aex-dev-euw1-stream-tg/73e2d6bc24d8a067"
    load_balancer_security_group_ids  = ["aex-dev-euw1-public-alb"]
    health_check_grace_period_seconds = 60
  }

  expect_failures = [var.load_balancer_security_group_ids]
}

run "rejects_a_load_balanced_service_that_names_no_load_balancer_group" {
  command = plan

  variables {
    target_group_arn                  = "arn:aws:elasticloadbalancing:eu-west-1:000000000000:targetgroup/aex-dev-euw1-stream-tg/73e2d6bc24d8a067"
    health_check_grace_period_seconds = 60
  }

  expect_failures = [var.load_balancer_security_group_ids]
}

run "rejects_a_load_balancer_group_on_a_service_behind_no_target_group" {
  command = plan

  variables {
    load_balancer_security_group_ids = ["sg-0123456789abcdef2"]
  }

  expect_failures = [var.load_balancer_security_group_ids]
}

run "rejects_more_than_one_load_balancer_group" {
  command = plan

  variables {
    target_group_arn                  = "arn:aws:elasticloadbalancing:eu-west-1:000000000000:targetgroup/aex-dev-euw1-stream-tg/73e2d6bc24d8a067"
    load_balancer_security_group_ids  = ["sg-0123456789abcdef2", "sg-0123456789abcdef3"]
    health_check_grace_period_seconds = 60
  }

  expect_failures = [var.load_balancer_security_group_ids]
}

run "rejects_an_additional_group_that_is_not_a_security_group" {
  command = plan

  variables {
    additional_security_group_ids = ["aex-dev-eu-west-1-session-api-task"]
  }

  expect_failures = [var.additional_security_group_ids]
}

# --- reached by name rather than through a load balancer ----------------------
#
# The shape the tool executor needs. The claim recorded against this module was
# that it *requires* a target group and that a private-only service therefore
# needed a new internal-ALB module. It does not, and these prove it: what a
# private service was actually missing was a stable address and an admitted
# caller, and both are declared rather than load balanced.

run "a_named_service_registers_with_cloud_map_and_no_load_balancer" {
  command = plan

  variables {
    name                      = "private-worker"
    task_definition_family    = "aex-dev-eu-west-1-private-worker"
    service_discovery_arn     = "arn:aws:servicediscovery:eu-west-1:000000000000:service/srv-0123456789abcdef"
    client_security_group_ids = ["sg-0123456789abcdef3"]
    autoscaling_metrics       = []
    autoscaling_bounds = {
      min_capacity = 1
      max_capacity = 1
    }
  }

  assert {
    condition     = length(aws_ecs_service.static[0].service_registries) == 1
    error_message = "A service with no load balancer has no address unless it registers a name."
  }

  assert {
    condition     = length(aws_ecs_service.static[0].load_balancer) == 0
    error_message = "A named service must not also be registered with a target group."
  }
}

run "a_named_service_admits_its_declared_client_and_nothing_else" {
  command = plan

  variables {
    name                      = "private-worker"
    task_definition_family    = "aex-dev-eu-west-1-private-worker"
    service_discovery_arn     = "arn:aws:servicediscovery:eu-west-1:000000000000:service/srv-0123456789abcdef"
    client_security_group_ids = ["sg-0123456789abcdef3"]
    autoscaling_metrics       = []
    autoscaling_bounds = {
      min_capacity = 1
      max_capacity = 1
    }
  }

  assert {
    condition     = length(aws_vpc_security_group_ingress_rule.from_client) == 1
    error_message = "The declared client is the one source a named service admits."
  }

  assert {
    condition     = aws_vpc_security_group_ingress_rule.from_client[0].referenced_security_group_id == "sg-0123456789abcdef3"
    error_message = "The admitted source must be the group the caller runs with, not a CIDR."
  }

  assert {
    condition     = aws_vpc_security_group_egress_rule.client_to_task[0].security_group_id == "sg-0123456789abcdef3"
    error_message = "The caller's own group needs the matching egress, or the caller reaches nothing and the service looks healthy."
  }

  assert {
    condition     = length(aws_vpc_security_group_ingress_rule.from_load_balancer) == 0
    error_message = "A named service admits no load balancer."
  }
}

run "a_named_service_with_no_declared_client_admits_nothing" {
  command = plan

  variables {
    name                      = "private-worker"
    task_definition_family    = "aex-dev-eu-west-1-private-worker"
    service_discovery_arn     = "arn:aws:servicediscovery:eu-west-1:000000000000:service/srv-0123456789abcdef"
    client_security_group_ids = []
    autoscaling_metrics       = []
    autoscaling_bounds = {
      min_capacity = 1
      max_capacity = 1
    }
  }

  assert {
    condition     = length(aws_vpc_security_group_ingress_rule.from_client) == 0
    error_message = "Naming no client must leave the tasks admitting nothing. Fail-closed is the only acceptable reading of an unspecified caller for a service that spends the platform's money."
  }
}

run "rejects_a_service_that_is_both_named_and_load_balanced" {
  command = plan

  variables {
    service_discovery_arn             = "arn:aws:servicediscovery:eu-west-1:000000000000:service/srv-0123456789abcdef"
    target_group_arn                  = "arn:aws:elasticloadbalancing:eu-west-1:000000000000:targetgroup/aex-dev-euw1-stream-tg/73e2d6bc24d8a067"
    load_balancer_security_group_ids  = ["sg-0123456789abcdef2"]
    health_check_grace_period_seconds = 60
  }

  expect_failures = [var.service_discovery_arn]
}

run "rejects_a_direct_client_on_a_load_balanced_service" {
  command = plan

  variables {
    client_security_group_ids         = ["sg-0123456789abcdef3"]
    target_group_arn                  = "arn:aws:elasticloadbalancing:eu-west-1:000000000000:targetgroup/aex-dev-euw1-stream-tg/73e2d6bc24d8a067"
    load_balancer_security_group_ids  = ["sg-0123456789abcdef2"]
    health_check_grace_period_seconds = 60
  }

  expect_failures = [var.client_security_group_ids]
}

run "rejects_a_service_discovery_entry_that_is_not_cloud_map" {
  command = plan

  variables {
    service_discovery_arn = "arn:aws:elasticloadbalancing:eu-west-1:000000000000:targetgroup/x/1"
  }

  expect_failures = [var.service_discovery_arn]
}
