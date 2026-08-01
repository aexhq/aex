variable "name" {
  type        = string
  description = "Name prefix for the VPC and everything in it."

  validation {
    condition     = can(regex("^aex-[a-z0-9-]{3,50}$", var.name))
    error_message = "The name must start with `aex-` and be lowercase and hyphen-separated."
  }
}

variable "region" {
  type        = string
  description = "AWS region, used to build gateway and interface endpoint service names."

  validation {
    condition     = can(regex("^[a-z]{2}-[a-z]+-[0-9]$", var.region))
    error_message = "The region must be an AWS region code such as `eu-west-1`."
  }
}

variable "cidr" {
  type        = string
  description = "VPC CIDR block."

  validation {
    condition     = can(cidrhost(var.cidr, 0))
    error_message = "The CIDR must be a valid IPv4 CIDR block."
  }

  validation {
    condition     = tonumber(split("/", var.cidr)[1]) >= 16 && tonumber(split("/", var.cidr)[1]) <= 20
    error_message = "The VPC prefix must be between /16 and /20."
  }
}

variable "az_count" {
  type        = number
  description = "Number of availability zones to spread subnets across."

  validation {
    condition     = var.az_count >= 2 && var.az_count <= 4
    error_message = "The zone count must be between 2 and 4."
  }
}

variable "availability_zones" {
  type        = list(string)
  description = "The zones to use, supplied by the root. The module does not query the account for them, because zone naming is per-account and a lookup would make the plan depend on which account it runs in."

  validation {
    condition     = alltrue([for z in var.availability_zones : can(regex("^[a-z]{2}-[a-z]+-[0-9][a-z]$", z))])
    error_message = "Every zone must be an availability zone name such as `eu-west-1a`."
  }

  validation {
    condition     = length(var.availability_zones) == var.az_count
    error_message = "The zone list must have exactly `az_count` entries."
  }
}

variable "endpoints" {
  type        = list(string)
  description = "Interface endpoint service short names, such as `kms` or `ecr.api`. Gateway endpoints for S3 and DynamoDB are always created."

  validation {
    condition     = alltrue([for e in var.endpoints : can(regex("^[a-z0-9][a-z0-9.-]*$", e))])
    error_message = "Every endpoint must be a service short name such as `kms` or `ecr.api`."
  }

  validation {
    condition     = length(setsubtract(var.required_interface_services, var.endpoints)) == 0
    error_message = "The endpoint list must include every service the schema-admin task needs; without them the task has no path to AWS from a private subnet."
  }
}

variable "required_interface_services" {
  type = list(string)
  default = [
    "ecr.api",
    "ecr.dkr",
    "kms",
    "logs",
    "secretsmanager",
    "sts",
  ]
  description = "Interface endpoints the one-shot schema-admin task cannot run without. `endpoints` must be a superset."

  validation {
    condition     = length(var.required_interface_services) > 0
    error_message = "The required service list must not be empty."
  }
}

variable "allow_nat" {
  type        = bool
  default     = false
  description = "Whether NAT gateways are created. They are not, unless a root says so explicitly: a NAT gateway is a standing hourly cost and a standing egress path, and interface endpoints cover everything the workloads here need."

  validation {
    condition     = var.allow_nat == false || var.nat_justification != null
    error_message = "Enabling NAT requires an explicit written justification in `nat_justification`."
  }
}

variable "nat_justification" {
  type        = string
  default     = null
  description = "Why this network needs a NAT gateway. Required whenever `allow_nat` is true."

  validation {
    condition     = var.nat_justification == null || length(coalesce(var.nat_justification, "")) >= 16
    error_message = "The justification must be a real sentence, at least 16 characters."
  }
}

variable "tags" {
  type        = map(string)
  default     = {}
  description = "Tags applied to every resource."
}
