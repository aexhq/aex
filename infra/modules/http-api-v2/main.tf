locals {
  authorizer_partition = split(":", var.authorizer_alias_arn)[1]
  authorizer_region    = split(":", var.authorizer_alias_arn)[3]

  route_method = {
    for operation, route in var.routes : operation => split(" ", route.route_key)[0]
  }
  route_path = {
    for operation, route in var.routes : operation => trimprefix(route.route_key, "${split(" ", route.route_key)[0]} ")
  }
  route_permission_path = {
    for operation, path in local.route_path : operation => replace(path, "/\\{[A-Za-z][A-Za-z0-9]*\\}/", "*")
  }

  access_log_format = jsonencode({
    accountId         = "$context.accountId"
    authorizerError   = "$context.authorizer.error"
    domainName        = "$context.domainName"
    errorMessage      = "$context.error.message"
    integrationError  = "$context.integrationErrorMessage"
    integrationStatus = "$context.integration.status"
    ip                = "$context.identity.sourceIp"
    protocol          = "$context.protocol"
    requestId         = "$context.requestId"
    requestTime       = "$context.requestTime"
    routeKey          = "$context.routeKey"
    status            = "$context.status"
  })
}

resource "aws_cloudwatch_log_group" "access" {
  name              = "/aws/apigateway/${var.name}"
  retention_in_days = var.access_log_retention_days
  kms_key_id        = var.access_log_kms_key_arn
  tags              = var.tags
}

resource "aws_apigatewayv2_api" "this" {
  name                         = var.name
  protocol_type                = "HTTP"
  disable_execute_api_endpoint = true
  tags                         = var.tags

  lifecycle {
    precondition {
      condition = alltrue([
        for owner in keys(var.integration_alias_arns) :
        contains([for route in values(var.routes) : route.integration], owner)
      ])
      error_message = "Every declared integration owner must own at least one route."
    }
  }
}

resource "aws_apigatewayv2_integration" "owner" {
  for_each = var.integration_alias_arns

  api_id                 = aws_apigatewayv2_api.this.id
  integration_type       = "AWS_PROXY"
  integration_method     = "POST"
  integration_uri        = each.value
  payload_format_version = "2.0"
  timeout_milliseconds   = 30000
}

resource "aws_apigatewayv2_authorizer" "request" {
  api_id                            = aws_apigatewayv2_api.this.id
  name                              = "${var.name}-request"
  authorizer_type                   = "REQUEST"
  authorizer_uri                    = "arn:${local.authorizer_partition}:apigateway:${local.authorizer_region}:lambda:path/2015-03-31/functions/${var.authorizer_alias_arn}/invocations"
  authorizer_payload_format_version = "2.0"
  # Half of MAX_CONTEXT_LIFETIME_MS (crates/aex-central-http/src/authorizer.rs):
  # the authorizer context is valid for exactly 30 s, so a 15 s gateway cache can
  # never serve a context past the lifetime the integrations re-check. Keyed on
  # the Authorization header, so one credential's cache entry cannot admit
  # another's request.
  authorizer_result_ttl_in_seconds = 15
  enable_simple_responses          = true
  identity_sources                 = ["$request.header.Authorization"]
}

resource "aws_apigatewayv2_route" "explicit" {
  for_each = var.routes

  api_id             = aws_apigatewayv2_api.this.id
  route_key          = each.value.route_key
  target             = "integrations/${aws_apigatewayv2_integration.owner[each.value.integration].id}"
  authorization_type = each.value.credentialed ? "CUSTOM" : "NONE"
  authorizer_id      = each.value.credentialed ? aws_apigatewayv2_authorizer.request.id : null
}

resource "aws_apigatewayv2_stage" "default" {
  api_id      = aws_apigatewayv2_api.this.id
  name        = "$default"
  auto_deploy = true
  tags        = var.tags

  access_log_settings {
    destination_arn = aws_cloudwatch_log_group.access.arn
    format          = local.access_log_format
  }

  # Keep API Gateway's default route settings absent. With the AWS provider,
  # declaring this block while omitting its throttle fields serializes a
  # 0/0 stage throttle, which rejects every otherwise valid route with 429.
  # The public API's application-level limits remain the source of truth.
}

resource "aws_apigatewayv2_domain_name" "this" {
  domain_name = var.domain_name

  domain_name_configuration {
    certificate_arn = var.certificate_arn
    endpoint_type   = "REGIONAL"
    security_policy = "TLS_1_2"
  }

  tags = var.tags
}

resource "aws_apigatewayv2_api_mapping" "root" {
  api_id      = aws_apigatewayv2_api.this.id
  domain_name = aws_apigatewayv2_domain_name.this.id
  stage       = aws_apigatewayv2_stage.default.id
}

resource "aws_lambda_permission" "route" {
  for_each = var.routes

  statement_id  = "AllowRoute${replace(title(replace(each.key, "_", " ")), " ", "")}"
  action        = "lambda:InvokeFunction"
  function_name = var.integration_alias_arns[each.value.integration]
  principal     = "apigateway.amazonaws.com"
  source_arn    = "${aws_apigatewayv2_api.this.execution_arn}/*/${local.route_method[each.key]}${local.route_permission_path[each.key]}"
}

resource "aws_lambda_permission" "authorizer" {
  statement_id  = "AllowRequestAuthorizer"
  action        = "lambda:InvokeFunction"
  function_name = var.authorizer_alias_arn
  principal     = "apigateway.amazonaws.com"
  source_arn    = "${aws_apigatewayv2_api.this.execution_arn}/authorizers/${aws_apigatewayv2_authorizer.request.id}"
}
