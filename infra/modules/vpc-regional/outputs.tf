output "vpc_id" {
  value       = aws_vpc.this.id
  description = "Id of the VPC."
}

output "subnet_ids" {
  value = {
    private = aws_subnet.private[*].id
    public  = aws_subnet.public[*].id
  }
  description = "Private and public subnet ids. Workloads use the private list."
}

output "endpoint_subnet_ids" {
  value       = local.endpoint_subnet_ids
  description = "The private subnets every interface endpoint places an interface in. A prefix of the private list, and shorter than it whenever `endpoint_az_count` trims the spread. Exposed so the standing endpoint-hour count is readable from state rather than inferred."
}

output "endpoint_ids" {
  value = merge(
    { for k, e in aws_vpc_endpoint.gateway : k => e.id },
    { for k, e in aws_vpc_endpoint.interface : k => e.id },
  )
  description = "Service short name to endpoint id, gateway and interface together."
}

output "gateway_endpoint_prefix_list_ids" {
  value       = { for k, e in aws_vpc_endpoint.gateway : k => e.prefix_list_id }
  description = "Gateway endpoint service to AWS-managed prefix-list id, for least-privilege workload egress rules."
}

output "interface_endpoint_security_group_id" {
  value       = aws_security_group.interface_endpoints.id
  description = "Security group attached to every interface endpoint; it admits TLS from this VPC only."
}

output "nat_gateway_ids" {
  value       = aws_nat_gateway.this[*].id
  description = "NAT gateway ids. Empty unless a root explicitly enabled NAT."
}
