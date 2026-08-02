# `http-api-v2`

An explicit-route API Gateway HTTP API v2 in front of qualified Lambda aliases.
It creates one integration per declared owner, one exact invoke permission per
route, one uncached REQUEST authorizer and a Regional custom-domain mapping.

The module deliberately cannot express a `$default` or `ANY` route. Every route
is under `/api`, names its owner, and states whether it is credentialed. A
credentialed route gets the one REQUEST authorizer; an anonymous route gets
neither an authorizer nor an `Authorization` identity source. The authorizer is
fixed to payload format 2.0, simple responses, a zero-second result TTL, and
`$request.header.Authorization` as its only identity source.

All Lambda targets are qualified alias ARNs. Each integration route receives a
permission scoped to its method and path, with path parameters translated to
single-segment wildcards. The authorizer permission is scoped to this API's one
authorizer id. The raw execute-api endpoint is disabled, access logging is
mandatory, and the module creates a Regional TLS 1.2 custom domain but leaves
the external DNS record to the environment owner.

## Inputs

| Name | Description |
| --- | --- |
| `name` | Physical API name. |
| `domain_name` | Canonical public hostname. |
| `certificate_arn` | In-region ACM certificate. |
| `integration_alias_arns` | Logical owner to qualified Lambda alias ARN. |
| `authorizer_alias_arn` | Qualified REQUEST-authorizer alias ARN. |
| `routes` | Operation id to route key, integration owner and credential flag. |
| `access_log_retention_days` | Mandatory CloudWatch retention. |
| `access_log_kms_key_arn` | KMS key for access logs. |
| `tags` | Resource tags. |

## Outputs

The module returns the API id and execution ARN, canonical URL, exact route and
owner maps, access-log group name, and a DNS handoff containing the custom-domain
host, Regional target and hosted-zone id.

## Not proved locally

Terraform tests prove resource shape and policy with a mocked AWS provider. A
deployed smoke test must still prove API Gateway invokes each alias, preserves
the authorizer context, writes access logs and serves the canonical certificate.
