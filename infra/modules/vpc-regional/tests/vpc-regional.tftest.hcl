mock_provider "aws" {}

override_resource {
  target          = aws_security_group.interface_endpoints
  override_during = plan
  values = {
    id = "sg-0123456789abcdef0"
  }
}

variables {
  name               = "aex-dev-euw1"
  region             = "eu-west-1"
  cidr               = "10.40.0.0/16"
  az_count           = 2
  availability_zones = ["eu-west-1a", "eu-west-1b"]

  endpoints = [
    "ecr.api",
    "ecr.dkr",
    "kms",
    "logs",
    "secretsmanager",
    "sts",
    "sqs",
  ]
}

run "no_nat_gateway_by_default" {
  command = plan

  assert {
    condition     = length(aws_nat_gateway.this) == 0
    error_message = "No NAT gateway may exist unless a root explicitly asks for one."
  }

  assert {
    condition     = length(aws_eip.nat) == 0
    error_message = "No elastic IP may be allocated for NAT unless NAT is enabled."
  }

  assert {
    condition     = length(aws_route.private_nat) == 0
    error_message = "No private default route may point at a NAT gateway when NAT is off."
  }
}

run "interface_endpoints_cover_everything_the_schema_admin_task_needs" {
  command = plan

  assert {
    condition = alltrue([
      for s in var.required_interface_services : contains(keys(aws_vpc_endpoint.interface), s)
    ])
    error_message = "Every service the schema-admin task needs must have an interface endpoint."
  }

  assert {
    condition = alltrue([
      for k, e in aws_vpc_endpoint.interface : e.private_dns_enabled
    ])
    error_message = "Every interface endpoint must enable private DNS, or the SDK will still resolve the public name."
  }

  assert {
    condition = alltrue([
      for k, e in aws_vpc_endpoint.interface :
      toset(e.security_group_ids) == toset([aws_security_group.interface_endpoints.id])
    ])
    error_message = "Every interface endpoint must use the dedicated endpoint security group rather than the VPC default group."
  }

  assert {
    condition = (
      aws_vpc_security_group_ingress_rule.interface_endpoints_https.ip_protocol == "tcp" &&
      aws_vpc_security_group_ingress_rule.interface_endpoints_https.from_port == 443 &&
      aws_vpc_security_group_ingress_rule.interface_endpoints_https.to_port == 443 &&
      aws_vpc_security_group_ingress_rule.interface_endpoints_https.cidr_ipv4 == var.cidr
    )
    error_message = "The endpoint group must admit only TLS from this VPC."
  }

  assert {
    condition = alltrue([
      for k in ["s3", "dynamodb"] : contains(keys(aws_vpc_endpoint.gateway), k)
    ])
    error_message = "Gateway endpoints for S3 and DynamoDB must always exist."
  }
}

run "private_subnets_never_auto_assign_a_public_address" {
  command = plan

  assert {
    condition = alltrue([
      for s in aws_subnet.private : s.map_public_ip_on_launch == false
    ])
    error_message = "A private subnet must not auto-assign public addresses."
  }

  assert {
    condition     = length(aws_subnet.private) == var.az_count
    error_message = "There must be one private subnet per availability zone."
  }
}

run "nat_is_possible_when_a_root_justifies_it" {
  command = plan

  variables {
    allow_nat         = true
    nat_justification = "Provider egress for the dev plane until the egress proxy lands."
  }

  assert {
    condition     = length(aws_nat_gateway.this) == var.az_count
    error_message = "Enabling NAT must create one gateway per zone."
  }
}

run "rejects_enabling_nat_with_no_justification" {
  command = plan

  variables {
    allow_nat = true
  }

  expect_failures = [var.allow_nat]
}

run "rejects_an_endpoint_list_missing_a_required_service" {
  command = plan

  variables {
    endpoints = ["kms", "logs"]
  }

  expect_failures = [var.endpoints]
}

run "rejects_a_zone_list_that_does_not_match_the_zone_count" {
  command = plan

  variables {
    availability_zones = ["eu-west-1a"]
  }

  expect_failures = [var.availability_zones]
}

run "rejects_an_oversized_vpc_prefix" {
  command = plan

  variables {
    cidr = "10.40.0.0/8"
  }

  expect_failures = [var.cidr]
}
