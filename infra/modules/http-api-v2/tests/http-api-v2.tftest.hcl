mock_provider "aws" {}

override_resource {
  target          = aws_apigatewayv2_api.this
  override_during = plan
  values = {
    id            = "central-api"
    execution_arn = "arn:aws:execute-api:eu-west-1:000000000000:central-api"
  }
}

override_resource {
  target          = aws_apigatewayv2_authorizer.request
  override_during = plan
  values = {
    id = "request-authorizer"
  }
}

override_resource {
  target          = aws_cloudwatch_log_group.access
  override_during = plan
  values = {
    arn = "arn:aws:logs:eu-west-1:000000000000:log-group:/aws/apigateway/aex-dev-central"
  }
}

override_resource {
  target          = aws_apigatewayv2_stage.default
  override_during = plan
  values = {
    id = "$default"
  }
}

variables {
  name                      = "aex-dev-central"
  domain_name               = "dev-api.aex.dev"
  certificate_arn           = "arn:aws:acm:eu-west-1:000000000000:certificate/00000000-0000-4000-8000-000000000000"
  access_log_kms_key_arn    = "arn:aws:kms:eu-west-1:000000000000:key/00000000-0000-4000-8000-000000000000"
  access_log_retention_days = 30

  integration_alias_arns = {
    identity = "arn:aws:lambda:eu-west-1:000000000000:function:aex-dev-central-identity-api:live"
    control  = "arn:aws:lambda:eu-west-1:000000000000:function:aex-dev-central-control-api:live"
    finance  = "arn:aws:lambda:eu-west-1:000000000000:function:aex-dev-finance-api:live"
  }
  authorizer_alias_arn = "arn:aws:lambda:eu-west-1:000000000000:function:aex-dev-central-authz:live"

  routes = {
    device_authorization_create = {
      route_key    = "POST /api/auth/device/authorizations"
      integration  = "identity"
      credentialed = false
    }
    device_token_create = {
      route_key    = "POST /api/auth/device/tokens"
      integration  = "identity"
      credentialed = false
    }
    organizations_list = {
      route_key    = "GET /api/organizations"
      integration  = "control"
      credentialed = true
    }
    billing_balance_get = {
      route_key    = "GET /api/billing/balance"
      integration  = "finance"
      credentialed = true
    }
    billing_statement_get = {
      route_key    = "GET /api/organizations/{organizationId}/billing/statements/{statementId}"
      integration  = "finance"
      credentialed = true
    }
  }
}

run "the_api_is_http_v2_and_has_no_execute_api_bypass" {
  command = plan

  assert {
    condition     = aws_apigatewayv2_api.this.protocol_type == "HTTP"
    error_message = "The public API must be an API Gateway HTTP API."
  }

  assert {
    condition     = aws_apigatewayv2_api.this.disable_execute_api_endpoint
    error_message = "The raw execute-api endpoint must not bypass the canonical domain."
  }
}

run "each_owner_has_one_alias_only_payload_v2_integration" {
  command = plan

  assert {
    condition     = length(aws_apigatewayv2_integration.owner) == 3
    error_message = "The fixture's three owners must produce exactly three integrations."
  }

  assert {
    condition = alltrue([
      for owner, integration in aws_apigatewayv2_integration.owner :
      integration.integration_uri == var.integration_alias_arns[owner]
      && integration.payload_format_version == "2.0"
      && integration.integration_type == "AWS_PROXY"
      && length(split(":", integration.integration_uri)) == 8
      && !endswith(integration.integration_uri, ":$LATEST")
    ])
    error_message = "Every owner integration must target exactly its qualified alias with payload v2."
  }
}

run "the_request_authorizer_is_exact_and_briefly_cached" {
  command = plan

  assert {
    condition = (
      aws_apigatewayv2_authorizer.request.authorizer_type == "REQUEST"
      && aws_apigatewayv2_authorizer.request.authorizer_payload_format_version == "2.0"
      && aws_apigatewayv2_authorizer.request.enable_simple_responses
      && length(aws_apigatewayv2_authorizer.request.identity_sources) == 1
      && one(aws_apigatewayv2_authorizer.request.identity_sources) == "$request.header.Authorization"
      && aws_apigatewayv2_authorizer.request.authorizer_result_ttl_in_seconds == 15
      && aws_apigatewayv2_authorizer.request.authorizer_result_ttl_in_seconds * 2 <= 30
      && strcontains(aws_apigatewayv2_authorizer.request.authorizer_uri, var.authorizer_alias_arn)
    )
    error_message = "The authorizer must be REQUEST payload v2, simple, cached for at most half the 30 s assertion validity, and sourced only from Authorization."
  }
}

run "anonymous_routes_have_no_authorizer_and_credentialed_routes_do" {
  command = plan

  assert {
    condition = alltrue([
      for operation in ["device_authorization_create", "device_token_create"] :
      aws_apigatewayv2_route.explicit[operation].authorization_type == "NONE"
      && aws_apigatewayv2_route.explicit[operation].authorizer_id == null
    ])
    error_message = "Anonymous device-flow routes must reach the Rust edge without an API Gateway authorizer."
  }

  assert {
    condition = alltrue([
      for operation in ["organizations_list", "billing_balance_get"] :
      aws_apigatewayv2_route.explicit[operation].authorization_type == "CUSTOM"
      && aws_apigatewayv2_route.explicit[operation].authorizer_id == aws_apigatewayv2_authorizer.request.id
    ])
    error_message = "Every credentialed route must use the one request authorizer."
  }
}

run "the_route_set_is_explicit_and_has_no_default_catchall" {
  command = plan

  assert {
    condition     = length(aws_apigatewayv2_route.explicit) == 5
    error_message = "The module must materialize one route per explicit operation."
  }

  assert {
    condition = alltrue([
      for route in values(aws_apigatewayv2_route.explicit) :
      route.route_key != "$default" && !startswith(route.route_key, "ANY ")
    ])
    error_message = "The public edge must have no default or ANY catchall."
  }
}

run "invoke_permissions_are_alias_qualified_and_route_exact" {
  command = plan

  assert {
    condition     = length(aws_lambda_permission.route) == 5
    error_message = "Every explicit route needs its own least-privilege invoke permission."
  }

  assert {
    condition = alltrue([
      for operation, permission in aws_lambda_permission.route :
      permission.function_name == var.integration_alias_arns[var.routes[operation].integration]
      && permission.principal == "apigateway.amazonaws.com"
      && !endswith(permission.function_name, ":$LATEST")
      && length(split(":", permission.function_name)) == 8
    ])
    error_message = "Route permissions must attach to the owned alias, never the function or $LATEST."
  }

  assert {
    condition = (
      endswith(aws_lambda_permission.route["organizations_list"].source_arn, "/*/GET/api/organizations")
      && endswith(aws_lambda_permission.route["billing_balance_get"].source_arn, "/*/GET/api/billing/balance")
      && endswith(aws_lambda_permission.route["billing_statement_get"].source_arn, "/*/GET/api/organizations/*/billing/statements/*")
    )
    error_message = "A route permission must scope to the exact method and path, with one segment wildcard per parameter."
  }

  assert {
    condition = (
      aws_lambda_permission.authorizer.function_name == var.authorizer_alias_arn
      && endswith(aws_lambda_permission.authorizer.source_arn, "/authorizers/${aws_apigatewayv2_authorizer.request.id}")
    )
    error_message = "Only this API's authorizer may invoke the authorizer alias."
  }
}

run "the_default_stage_has_a_positive_outer_throttle_and_logs_every_request" {
  command = plan

  assert {
    condition = (
      aws_apigatewayv2_stage.default.name == "$default"
      && aws_apigatewayv2_stage.default.auto_deploy
      && one(aws_apigatewayv2_stage.default.default_route_settings).detailed_metrics_enabled == false
      && one(aws_apigatewayv2_stage.default.default_route_settings).throttling_burst_limit == 100
      && one(aws_apigatewayv2_stage.default.default_route_settings).throttling_rate_limit == 50
      && one(aws_apigatewayv2_stage.default.access_log_settings).destination_arn == aws_cloudwatch_log_group.access.arn
      && strcontains(one(aws_apigatewayv2_stage.default.access_log_settings).format, "$context.requestId")
      && strcontains(one(aws_apigatewayv2_stage.default.access_log_settings).format, "$context.routeKey")
      && strcontains(one(aws_apigatewayv2_stage.default.access_log_settings).format, "$context.status")
    )
    error_message = "The auto-deployed default stage must keep detailed metrics off, permit a bounded 100/50 outer throttle, and write access logs."
  }

  assert {
    condition = (
      aws_cloudwatch_log_group.access.kms_key_id == var.access_log_kms_key_arn
      && aws_cloudwatch_log_group.access.retention_in_days == var.access_log_retention_days
    )
    error_message = "The access-log group must use the declared customer key and retention."
  }
}

run "the_regional_custom_domain_maps_only_the_default_stage" {
  command = plan

  assert {
    condition = (
      aws_apigatewayv2_domain_name.this.domain_name == var.domain_name
      && one(aws_apigatewayv2_domain_name.this.domain_name_configuration).endpoint_type == "REGIONAL"
      && one(aws_apigatewayv2_domain_name.this.domain_name_configuration).security_policy == "TLS_1_2"
      && one(aws_apigatewayv2_domain_name.this.domain_name_configuration).certificate_arn == var.certificate_arn
      && aws_apigatewayv2_api_mapping.root.stage == aws_apigatewayv2_stage.default.name
    )
    error_message = "The canonical domain must be a Regional TLS 1.2 mapping of the default stage."
  }
}

run "rejects_an_unqualified_integration_function" {
  command = plan

  variables {
    integration_alias_arns = {
      identity = "arn:aws:lambda:eu-west-1:000000000000:function:aex-dev-central-identity-api"
      control  = "arn:aws:lambda:eu-west-1:000000000000:function:aex-dev-central-control-api:live"
      finance  = "arn:aws:lambda:eu-west-1:000000000000:function:aex-dev-finance-api:live"
    }
  }

  expect_failures = [var.integration_alias_arns]
}

run "rejects_latest_as_the_authorizer" {
  command = plan

  variables {
    authorizer_alias_arn = "arn:aws:lambda:eu-west-1:000000000000:function:aex-dev-central-authz:$LATEST"
  }

  expect_failures = [var.authorizer_alias_arn]
}

run "rejects_a_default_route" {
  command = plan

  variables {
    routes = {
      catchall = {
        route_key    = "$default"
        integration  = "control"
        credentialed = true
      }
    }
  }

  expect_failures = [var.routes]
}
