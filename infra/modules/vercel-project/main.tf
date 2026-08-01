locals {
  # dev and prd are always separate Vercel projects. Sharing one project across
  # planes would put a preview deployment one promotion away from production.
  full_project_name = "${var.project_name}-${var.plane}"
}

resource "vercel_project" "this" {
  name      = local.full_project_name
  team_id   = var.team_id
  framework = var.framework

  # The deployment is a prebuilt Build Output API tree. Nothing is built here.
  build_command   = var.build_command
  install_command = null
  dev_command     = null
  ignore_command  = null

  auto_assign_custom_domains = var.auto_assign_domains

  resource_config = {
    function_default_regions = [var.serverless_function_region]
  }

  automatically_expose_system_environment_variables = false
}

resource "vercel_project_domain" "this" {
  project_id = vercel_project.this.id
  team_id    = var.team_id
  domain     = var.domain
}

resource "vercel_project_environment_variable" "this" {
  for_each = { for e in var.env_vars : e.key => e }

  project_id = vercel_project.this.id
  team_id    = var.team_id
  key        = each.value.key
  value      = each.value.reference
  target     = each.value.target
  comment    = each.value.comment
  sensitive  = true
}
