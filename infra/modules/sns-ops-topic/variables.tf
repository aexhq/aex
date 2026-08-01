variable "name" {
  type        = string
  description = "Topic name."

  validation {
    condition     = can(regex("^aex-[a-z0-9-]{3,50}$", var.name))
    error_message = "The topic name must start with `aex-` and be lowercase and hyphen-separated."
  }
}

variable "region" {
  type        = string
  description = "AWS region, used to construct the topic ARN for the policy."

  validation {
    condition     = can(regex("^[a-z]{2}-[a-z]+-[0-9]$", var.region))
    error_message = "The region must be an AWS region code such as `eu-west-1`."
  }
}

variable "account_id" {
  type        = string
  description = "Account the topic lives in, used to construct the topic ARN for the policy. Supplied by the root; the module never discovers it."

  validation {
    condition     = can(regex("^[0-9A-Za-z-]{1,64}$", var.account_id))
    error_message = "The account id must be an account identifier."
  }
}

variable "partition" {
  type        = string
  default     = "aws"
  description = "AWS partition used to construct the topic ARN."

  validation {
    condition     = can(regex("^aws[a-z-]*$", var.partition))
    error_message = "The partition must be `aws` or an `aws-` prefixed partition name."
  }
}

variable "publish_principals" {
  type = list(object({
    type       = string
    identifier = string
  }))
  description = "Who may publish to the topic. An operational topic feeds a pager, so an open publish policy is an open pager."

  validation {
    condition     = length(var.publish_principals) > 0
    error_message = "At least one publish principal is required."
  }

  validation {
    condition     = alltrue([for p in var.publish_principals : contains(["AWS", "Service"], p.type)])
    error_message = "Every publish principal type must be `AWS` or `Service`."
  }

  validation {
    condition     = alltrue([for p in var.publish_principals : p.identifier != "*" && !strcontains(p.identifier, "*")])
    error_message = "A wildcard publish principal is not allowed; name every publisher explicitly."
  }
}

variable "subscriptions" {
  type = list(object({
    protocol = string
    endpoint = string
  }))
  default     = []
  description = "Subscriptions created on the topic."

  validation {
    condition     = alltrue([for s in var.subscriptions : contains(["email", "https", "sqs", "lambda"], s.protocol)])
    error_message = "The subscription protocol must be `email`, `https`, `sqs` or `lambda`."
  }

  validation {
    condition     = alltrue([for s in var.subscriptions : length(s.endpoint) > 0])
    error_message = "Every subscription must name an endpoint."
  }

  validation {
    condition = alltrue([
      for s in var.subscriptions : s.protocol != "https" || startswith(s.endpoint, "https://")
    ])
    error_message = "An https subscription endpoint must be an https URL."
  }
}

variable "kms_key_arn" {
  type        = string
  default     = null
  description = "Optional customer-managed key for the topic."

  validation {
    condition     = var.kms_key_arn == null || can(regex("^arn:aws[a-z-]*:kms:", coalesce(var.kms_key_arn, "none")))
    error_message = "When set, the key must be a KMS key ARN."
  }
}

variable "tags" {
  type        = map(string)
  default     = {}
  description = "Tags applied to the topic."
}
