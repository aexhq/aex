# `vpc-regional`

One regional VPC: private and public subnets across the zones the root names,
gateway endpoints for S3 and DynamoDB, and interface endpoints for everything
else.

There is no NAT gateway. A NAT gateway is a standing hourly cost and a standing
egress path, and the workloads here reach AWS through interface endpoints. A
root that genuinely needs one sets `allow_nat` **and** writes down why in
`nat_justification`; enabling it without a justification is rejected.

The zone names are an input. Zone naming is per-account - one account's
`eu-west-1a` is not another's - so a lookup would make the plan depend on which
account it happens to run in.

## Inputs

| Name | Type | Description |
| --- | --- | --- |
| `name` | `string` | Name prefix, `aex-<...>`. |
| `region` | `string` | Region, for endpoint service names. |
| `cidr` | `string` | VPC CIDR, /16 to /20. |
| `az_count` | `number` | 2 to 4 zones. |
| `availability_zones` | `list(string)` | Exactly `az_count` zone names. |
| `endpoints` | `list(string)` | Interface endpoint service short names. |
| `required_interface_services` | `list(string)` | Services the schema-admin task cannot run without. |
| `allow_nat` | `bool` | Defaults to `false`. |
| `nat_justification` | `string` | Required when `allow_nat` is true. |
| `tags` | `map(string)` | Tags. |

## Outputs

| Name | Description |
| --- | --- |
| `vpc_id` | Id of the VPC. |
| `subnet_ids` | `{ private, public }` subnet id lists. |
| `endpoint_ids` | Service short name to endpoint id. |
| `interface_endpoint_security_group_id` | Endpoint-only group admitting TCP/443 from this VPC. |
| `nat_gateway_ids` | NAT gateway ids; empty unless NAT was enabled. |

## Policy asserted

- No NAT gateway, elastic IP or NAT default route exists unless `allow_nat` is
  explicitly true, and enabling it without a written justification is rejected.
- `endpoints` must be a superset of `required_interface_services`, so the
  one-shot schema-admin task always has a path to ECR, KMS, CloudWatch Logs,
  Secrets Manager and STS from a private subnet.
- Every interface endpoint enables private DNS; without it the SDK still
  resolves the public name and the endpoint does nothing. Every endpoint also
  uses a dedicated security group admitting only TCP/443 from this VPC; it
  never inherits the default security group's self-reference.
- Gateway endpoints for S3 and DynamoDB always exist.
- Private subnets never auto-assign a public address, and there is one per zone.
- The zone list must match `az_count`, and the VPC prefix must be between /16
  and /20.

## Not asserted here

Reachability is proven by the `net.public_egress` seam and by each deployable's
own live suite inside the network.
