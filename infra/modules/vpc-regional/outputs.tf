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

output "endpoint_ids" {
  value = merge(
    { for k, e in aws_vpc_endpoint.gateway : k => e.id },
    { for k, e in aws_vpc_endpoint.interface : k => e.id },
  )
  description = "Service short name to endpoint id, gateway and interface together."
}

output "interface_endpoint_security_group_id" {
  value       = aws_security_group.interface_endpoints.id
  description = "Security group attached to every interface endpoint; it admits TLS from this VPC only."
}

output "nat_gateway_ids" {
  value       = aws_nat_gateway.this[*].id
  description = "NAT gateway ids. Empty unless a root explicitly enabled NAT."
}
