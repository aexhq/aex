variable "namespace" {
  type        = string
  description = "The private DNS namespace services are named inside, such as `aex-dev.internal`. It resolves only from the VPC it is attached to."

  validation {
    condition     = can(regex("^[a-z][a-z0-9-]{1,40}(\.[a-z][a-z0-9-]{1,40})+$", var.namespace))
    error_message = "The namespace must be a lowercase dotted DNS name such as `aex-dev.internal`."
  }
}

variable "vpc_id" {
  type        = string
  description = "The VPC the namespace resolves in. A private namespace is meaningless outside one, which is what makes a name here unreachable from a customer sandbox."

  validation {
    condition     = can(regex("^vpc-[0-9a-f]{8,17}$", var.vpc_id))
    error_message = "The VPC must be an EC2 VPC id such as `vpc-0123456789abcdef0`."
  }
}

variable "services" {
  type        = list(string)
  description = "The service names registered in the namespace. Each becomes `<name>.<namespace>`."

  validation {
    condition     = length(var.services) > 0
    error_message = "A namespace with no services registers nothing and should not be created."
  }

  validation {
    condition     = alltrue([for name in var.services : can(regex("^[a-z][a-z0-9-]{2,50}$", name))])
    error_message = "Every service name must be lowercase and hyphen-separated, matching the deployable id it names."
  }

  validation {
    condition     = length(distinct(var.services)) == length(var.services)
    error_message = "Two services cannot claim one name; the second would silently replace the first."
  }
}

variable "record_ttl" {
  type        = number
  default     = 10
  description = "Seconds a resolver may cache the record. Short, because the addresses are task addresses and a deployment replaces them."

  validation {
    condition     = var.record_ttl >= 5 && var.record_ttl <= 60
    error_message = "The record TTL must be between 5 and 60 seconds. Longer and a caller keeps dialling a task that has stopped; shorter and every connection pays for a resolution."
  }
}

variable "tags" {
  type        = map(string)
  default     = {}
  description = "Tags applied to the namespace and every service in it."
}
