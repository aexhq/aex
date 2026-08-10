mock_provider "aws" {}

variables {
  namespace = "aex-dev.internal"
  vpc_id    = "vpc-0123456789abcdef0"
  services  = ["tool-executor"]
}

run "the_namespace_is_private_to_one_vpc" {
  command = plan

  assert {
    condition     = aws_service_discovery_private_dns_namespace.this.vpc == "vpc-0123456789abcdef0"
    error_message = "A namespace attached to no VPC, or to the wrong one, is a name a customer sandbox could resolve."
  }
}

run "every_service_resolves_to_the_addresses_of_its_own_tasks" {
  command = plan

  assert {
    condition     = aws_service_discovery_service.this["tool-executor"].dns_config[0].dns_records[0].type == "A"
    error_message = "An `awsvpc` task has its own address, so the record is an A record. An alias would point at a load balancer this service does not have."
  }

  assert {
    condition     = aws_service_discovery_service.this["tool-executor"].dns_config[0].routing_policy == "MULTIVALUE"
    error_message = "A weighted policy hands out one task at a time and pins a long-lived connection pool to whichever answered first."
  }
}

run "rejects_a_namespace_that_is_not_a_dotted_dns_name" {
  command = plan

  variables {
    namespace = "aex-dev"
  }

  expect_failures = [var.namespace]
}

run "rejects_a_vpc_id_that_is_not_a_vpc" {
  command = plan

  variables {
    vpc_id = "aex-dev-vpc"
  }

  expect_failures = [var.vpc_id]
}

run "rejects_a_namespace_that_registers_nothing" {
  command = plan

  variables {
    services = []
  }

  expect_failures = [var.services]
}

run "rejects_two_services_claiming_one_name" {
  command = plan

  variables {
    services = ["tool-executor", "tool-executor"]
  }

  expect_failures = [var.services]
}

run "rejects_a_record_ttl_long_enough_to_outlive_a_deployment" {
  command = plan

  variables {
    record_ttl = 300
  }

  expect_failures = [var.record_ttl]
}
