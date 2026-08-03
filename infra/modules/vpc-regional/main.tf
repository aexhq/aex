locals {
  private_cidrs = [
    for i in range(var.az_count) : cidrsubnet(var.cidr, 4, i)
  ]

  public_cidrs = [
    for i in range(var.az_count) : cidrsubnet(var.cidr, 4, i + 8)
  ]

  nat_count = var.allow_nat ? var.az_count : 0
}

resource "aws_vpc" "this" {
  cidr_block           = var.cidr
  enable_dns_support   = true
  enable_dns_hostnames = true

  tags = merge(var.tags, { Name = var.name })
}

resource "aws_subnet" "private" {
  count = var.az_count

  vpc_id                  = aws_vpc.this.id
  cidr_block              = local.private_cidrs[count.index]
  availability_zone       = var.availability_zones[count.index]
  map_public_ip_on_launch = false

  tags = merge(var.tags, { Name = "${var.name}-private-${count.index}" })
}

resource "aws_subnet" "public" {
  count = var.az_count

  vpc_id                  = aws_vpc.this.id
  cidr_block              = local.public_cidrs[count.index]
  availability_zone       = var.availability_zones[count.index]
  map_public_ip_on_launch = false

  tags = merge(var.tags, { Name = "${var.name}-public-${count.index}" })
}

resource "aws_internet_gateway" "this" {
  vpc_id = aws_vpc.this.id

  tags = merge(var.tags, { Name = var.name })
}

resource "aws_route_table" "public" {
  vpc_id = aws_vpc.this.id

  tags = merge(var.tags, { Name = "${var.name}-public" })
}

resource "aws_route" "public_default" {
  route_table_id         = aws_route_table.public.id
  destination_cidr_block = "0.0.0.0/0"
  gateway_id             = aws_internet_gateway.this.id
}

resource "aws_route_table_association" "public" {
  count = var.az_count

  subnet_id      = aws_subnet.public[count.index].id
  route_table_id = aws_route_table.public.id
}

resource "aws_route_table" "private" {
  count = var.az_count

  vpc_id = aws_vpc.this.id

  tags = merge(var.tags, { Name = "${var.name}-private-${count.index}" })
}

resource "aws_route_table_association" "private" {
  count = var.az_count

  subnet_id      = aws_subnet.private[count.index].id
  route_table_id = aws_route_table.private[count.index].id
}

# No NAT gateway exists unless a root sets `allow_nat` and writes down why.
# Everything the regional workloads reach is covered by the endpoints below.
resource "aws_eip" "nat" {
  count = local.nat_count

  domain = "vpc"
  tags   = merge(var.tags, { Name = "${var.name}-nat-${count.index}" })
}

resource "aws_nat_gateway" "this" {
  count = local.nat_count

  allocation_id = aws_eip.nat[count.index].id
  subnet_id     = aws_subnet.public[count.index].id

  tags = merge(var.tags, { Name = "${var.name}-nat-${count.index}" })
}

resource "aws_route" "private_nat" {
  count = local.nat_count

  route_table_id         = aws_route_table.private[count.index].id
  destination_cidr_block = "0.0.0.0/0"
  nat_gateway_id         = aws_nat_gateway.this[count.index].id
}

resource "aws_vpc_endpoint" "gateway" {
  for_each = toset(["s3", "dynamodb"])

  vpc_id            = aws_vpc.this.id
  service_name      = "com.amazonaws.${var.region}.${each.key}"
  vpc_endpoint_type = "Gateway"
  route_table_ids   = aws_route_table.private[*].id

  tags = merge(var.tags, { Name = "${var.name}-${each.key}" })
}

# Interface endpoints must never inherit the VPC default security group. The
# workloads use dedicated groups, so the default group's self-reference would
# resolve the private AWS hostname successfully and then reject the TLS
# connection. This endpoint-only group admits exactly TCP/443 from addresses in
# this VPC; it grants no public ingress and no additional destination port.
resource "aws_security_group" "interface_endpoints" {
  name        = "${var.name}-interface-endpoints"
  description = "TLS ingress to private AWS interface endpoints"
  vpc_id      = aws_vpc.this.id

  tags = merge(var.tags, { Name = "${var.name}-interface-endpoints" })
}

resource "aws_vpc_security_group_ingress_rule" "interface_endpoints_https" {
  security_group_id = aws_security_group.interface_endpoints.id
  description       = "TLS from workloads inside this VPC"
  ip_protocol       = "tcp"
  from_port         = 443
  to_port           = 443
  cidr_ipv4         = var.cidr
}

resource "aws_vpc_endpoint" "interface" {
  for_each = toset(var.endpoints)

  vpc_id              = aws_vpc.this.id
  service_name        = "com.amazonaws.${var.region}.${each.key}"
  vpc_endpoint_type   = "Interface"
  subnet_ids          = aws_subnet.private[*].id
  security_group_ids  = [aws_security_group.interface_endpoints.id]
  private_dns_enabled = true

  tags = merge(var.tags, { Name = "${var.name}-${replace(each.key, ".", "-")}" })
}
