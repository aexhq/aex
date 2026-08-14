output "vpc_id" {
  value       = module.network.vpc_id
  description = "Id of the regional VPC."
}

output "private_subnet_ids" {
  value       = module.network.subnet_ids.private
  description = "Private subnets every workload runs in."
}

output "public_subnet_ids" {
  value       = module.network.subnet_ids.public
  description = "Public subnets the regional load balancer sits in."
}

output "interface_endpoint_security_group_id" {
  value       = module.network.interface_endpoint_security_group_id
  description = "The group every private AWS interface endpoint shares. Regional tasks use these endpoints for AWS APIs while only Brain and Tool Mux have public TLS egress."
}

output "nat_gateway_ids" {
  value       = module.network.nat_gateway_ids
  description = "NAT gateways providing the private-subnet route used by bounded Brain/Tool public TLS egress."
}

output "gateway_endpoint_prefix_list_ids" {
  value       = module.network.gateway_endpoint_prefix_list_ids
  description = "Gateway endpoint service to AWS-managed prefix-list id. A gateway endpoint is a route rather than an interface, so a task's egress rule can only name it through its prefix list."
}

output "authority_key_arns" {
  value       = { for k, m in module.authority_key : k => m.key_arn }
  description = "Authority to customer-managed key ARN."
}

output "table_names" {
  value       = module.tables.table_names
  description = "Logical table name to physical table name."
}

output "stream_arns" {
  value       = module.tables.stream_arns
  description = "Logical table name to stream ARN."
}

output "content_bucket" {
  value       = module.content.bucket
  description = "Regional content bucket."
}
