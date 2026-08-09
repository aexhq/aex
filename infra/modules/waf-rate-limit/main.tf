# A per-source-IP rate limit in front of a named set of exact request paths.
#
# # Why this module exists
#
# `infra/modules/http-api-v2` carried the only request throttle anywhere in the
# central plane (`throttling_burst_limit = 100`, `throttling_rate_limit = 50`).
# API Gateway cannot integrate an ECS service, so moving the central surface
# behind an Application Load Balancer removes that throttle, and an ALB has no
# throttling of any kind. This is its replacement, and it is deliberately not a
# like-for-like one: a rate-based rule counts per source IP over a fixed window
# rather than expressing "50 requests per second overall".
#
# # Why it is scoped down rather than plane-wide
#
# Every authenticated central route now resolves its credential against Aurora on
# every request, because the authorizer's 15 s result cache went with the
# authorizer. The routes that carry no credential at all are the two device-flow
# routes, and they are also the ones where every anonymous caller shares a single
# replay principal. Those are what the limit is pointed at. A plane-wide limit
# would spend the same money throttling the paying traffic instead.
#
# # What is deliberately not here
#
# No managed rule groups, no bot control, no logging configuration. Each is a
# separate decision with a separate bill, and a module that quietly enabled one
# would make the cost of "add a rate limit" something other than what it says.

locals {
  # One `byte_match_statement` per path, OR'd together, because a rate-based
  # rule takes exactly one scope-down statement. `EXACTLY` rather than
  # `STARTS_WITH`: a prefix on `/api/auth/device/` would also cover any route
  # added under it later, and inheriting a throttle silently is how a route ends
  # up rate limited by a rule nobody wrote for it.
  path_statements = [
    for path in var.paths : {
      search_string = path
    }
  ]
}

resource "aws_wafv2_web_acl" "this" {
  name        = var.name
  description = "Per-source-IP rate limit for ${length(var.paths)} unauthenticated path(s) on ${var.name}"
  scope       = "REGIONAL"
  tags        = var.tags

  # Allow by default. This ACL exists to bound one class of caller, not to become
  # the plane's admission control: every authorization decision belongs to the
  # service behind it, and a default block here would make an outage of this
  # rule an outage of the whole listener.
  default_action {
    allow {}
  }

  rule {
    name     = "${var.name}-rate"
    priority = 0

    action {
      block {}
    }

    statement {
      rate_based_statement {
        limit                 = var.rate_limit
        evaluation_window_sec = var.evaluation_window_seconds

        # `IP` rather than `FORWARDED_IP`. The ALB sets `X-Forwarded-For` from
        # the connection it accepted, but a caller may append to it, so trusting
        # the header would let one source present a fresh key per request and
        # buy itself an unbounded rate.
        aggregate_key_type = "IP"

        scope_down_statement {
          or_statement {
            dynamic "statement" {
              for_each = local.path_statements

              content {
                byte_match_statement {
                  positional_constraint = "EXACTLY"
                  search_string         = statement.value.search_string

                  field_to_match {
                    uri_path {}
                  }

                  # The path is compared after the same normalisation the
                  # service's router applies, so `/api/auth//device/tokens` and a
                  # percent-encoded spelling cannot walk around the rule.
                  text_transformation {
                    priority = 0
                    type     = "URL_DECODE"
                  }

                  text_transformation {
                    priority = 1
                    type     = "COMPRESS_WHITE_SPACE"
                  }
                }
              }
            }
          }
        }
      }
    }

    visibility_config {
      cloudwatch_metrics_enabled = true
      metric_name                = "${var.name}-rate"
      sampled_requests_enabled   = false
    }
  }

  # `sampled_requests_enabled = false` throughout: a sampled request is stored
  # with its headers, and the paths this rule watches carry device codes.
  visibility_config {
    cloudwatch_metrics_enabled = true
    metric_name                = var.name
    sampled_requests_enabled   = false
  }
}

resource "aws_wafv2_web_acl_association" "this" {
  resource_arn = var.resource_arn
  web_acl_arn  = aws_wafv2_web_acl.this.arn
}
