# The name a private-only service is reached by.
#
# `infra/modules/ecs-service` requires no load balancer — its `target_group_arn`
# defaults to null and the `load_balancer` block is conditional — so a service
# with no public edge deploys today. What it did not have was an address: a
# Fargate task's IP changes on every deployment, so a caller needs a name.
#
# Cloud Map answers that with a DNS record and nothing else. The alternative
# considered and rejected was a new internal-ALB module: for a single-task
# service with exactly one caller it adds a listener, a target group, two more
# security-group edges and a monthly bill, to solve a name-resolution problem
# rather than a load-distribution one. The `alb-public` module in this tree
# hardcodes `internal = false`, so it could not have been reused either way.

resource "aws_service_discovery_private_dns_namespace" "this" {
  name        = var.namespace
  description = "Private names for services in ${var.namespace} that are reached inside the VPC rather than through a load balancer"
  vpc         = var.vpc_id
  tags        = var.tags
}

resource "aws_service_discovery_service" "this" {
  for_each = toset(var.services)

  name = each.value

  dns_config {
    namespace_id = aws_service_discovery_private_dns_namespace.this.id

    dns_records {
      # An A record per task rather than an alias or an SRV record: `awsvpc`
      # gives every task its own address, and a caller that resolves the name
      # gets the addresses of the tasks that are actually registered.
      ttl  = var.record_ttl
      type = "A"
    }

    # `MULTIVALUE` rather than `WEIGHTED`: with several tasks a resolver
    # receives all of them and picks, which is what a client-side connection
    # pool wants. `WEIGHTED` would hand out one at a time and pin a long-lived
    # pool to whichever task answered first.
    routing_policy = "MULTIVALUE"
  }

  # ECS is the only writer of instance health here: it registers a task when the
  # task starts and deregisters it when it stops. The block is declared with no
  # arguments because `failure_threshold` is deprecated -- AWS pins it to 1 and
  # the provider warns on any value -- and its absence would make this a
  # Route 53-health-checked service, which is not what an ECS-managed
  # registration is.
  health_check_custom_config {}

  tags = var.tags
}
