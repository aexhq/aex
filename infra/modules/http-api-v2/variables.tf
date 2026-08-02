variable "name" {
  type        = string
  description = "Physical HTTP API name."

  validation {
    condition     = can(regex("^aex-[a-z0-9-]{3,60}$", var.name))
    error_message = "The API name must start with `aex-` and be lowercase and hyphen-separated."
  }
}

variable "domain_name" {
  type        = string
  description = "Canonical public hostname. The raw execute-api endpoint is disabled."

  validation {
    condition     = can(regex("^[a-z0-9](?:[a-z0-9-]{0,62}\\.)+[a-z]{2,63}$", var.domain_name))
    error_message = "The domain name must be a lowercase fully-qualified hostname."
  }
}

variable "certificate_arn" {
  type        = string
  description = "In-region ACM certificate for the Regional custom domain."

  validation {
    condition     = can(regex("^arn:aws[a-z-]*:acm:[a-z0-9-]+:[0-9]{12}:certificate/[0-9a-f-]+$", var.certificate_arn))
    error_message = "The certificate must be an ACM certificate ARN."
  }
}

variable "integration_alias_arns" {
  type        = map(string)
  description = "Logical integration owner to qualified Lambda alias ARN. Unqualified functions, versions and `$LATEST` are refused."

  validation {
    condition     = length(var.integration_alias_arns) > 0
    error_message = "At least one integration owner is required."
  }

  validation {
    condition = alltrue([
      for owner, arn in var.integration_alias_arns :
      can(regex("^[a-z][a-z0-9_-]{1,31}$", owner))
      && can(regex("^arn:aws[a-z-]*:lambda:[a-z0-9-]+:[0-9]{12}:function:[A-Za-z0-9-_]+:[A-Za-z][A-Za-z0-9-_]{0,127}$", arn))
      && !endswith(arn, ":$LATEST")
      && length(split(":", arn)) == 8
    ])
    error_message = "Every integration must have a stable logical owner and a qualified, non-$LATEST Lambda alias ARN."
  }

}

variable "authorizer_alias_arn" {
  type        = string
  description = "Qualified Lambda alias ARN of the REQUEST authorizer."

  validation {
    condition = (
      can(regex("^arn:aws[a-z-]*:lambda:[a-z0-9-]+:[0-9]{12}:function:[A-Za-z0-9-_]+:[A-Za-z][A-Za-z0-9-_]{0,127}$", var.authorizer_alias_arn))
      && !endswith(var.authorizer_alias_arn, ":$LATEST")
      && length(split(":", var.authorizer_alias_arn)) == 8
    )
    error_message = "The authorizer must be a qualified, non-$LATEST Lambda alias ARN."
  }

  validation {
    condition = alltrue([
      for arn in values(var.integration_alias_arns) :
      split(":", arn)[1] == split(":", var.authorizer_alias_arn)[1]
      && split(":", arn)[3] == split(":", var.authorizer_alias_arn)[3]
      && split(":", arn)[4] == split(":", var.authorizer_alias_arn)[4]
    ])
    error_message = "The authorizer and every integration alias must share one partition, region and account."
  }
}

variable "routes" {
  type = map(object({
    route_key    = string
    integration  = string
    credentialed = bool
  }))
  description = "Operation id to exact method/path, integration owner and credential requirement. There is no default route."

  validation {
    condition     = length(var.routes) > 0
    error_message = "At least one explicit route is required."
  }

  validation {
    condition = alltrue([
      for operation, route in var.routes :
      can(regex("^[a-z][a-z0-9_]{1,95}$", operation))
      && can(regex("^(DELETE|GET|PATCH|POST|PUT) /api(?:/[A-Za-z0-9._~-]+|/\\{[A-Za-z][A-Za-z0-9]*\\})+$", route.route_key))
      && route.route_key != "$default"
      && !startswith(route.route_key, "ANY ")
      && !strcontains(route.route_key, "*")
      && contains(keys(var.integration_alias_arns), route.integration)
    ])
    error_message = "Every route must have a stable operation id, one explicit supported method under `/api`, no wildcard, and a declared integration owner."
  }

  validation {
    condition     = length(distinct([for route in values(var.routes) : route.route_key])) == length(var.routes)
    error_message = "Two operations may not declare the same method and path."
  }
}

variable "access_log_retention_days" {
  type        = number
  description = "CloudWatch access-log retention. Access logging is mandatory."

  validation {
    condition = contains(
      [1, 3, 5, 7, 14, 30, 60, 90, 120, 150, 180, 365, 400, 545, 731, 1096, 1827, 2192, 2557, 2922, 3288, 3653],
      var.access_log_retention_days
    )
    error_message = "The retention must be one of the CloudWatch retention values."
  }
}

variable "access_log_kms_key_arn" {
  type        = string
  description = "Customer-managed KMS key for the access-log group."

  validation {
    condition     = can(regex("^arn:aws[a-z-]*:kms:[a-z0-9-]+:[0-9]{12}:key/[0-9a-f-]+$", var.access_log_kms_key_arn))
    error_message = "The access-log key must be a KMS key ARN."
  }
}

variable "tags" {
  type        = map(string)
  default     = {}
  description = "Tags applied to the API, stage, domain and access-log group."
}
