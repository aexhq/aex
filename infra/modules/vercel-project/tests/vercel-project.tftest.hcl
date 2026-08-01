mock_provider "vercel" {}

variables {
  team_id      = "team-placeholder"
  project_name = "aex-dashboard"
  plane        = "dev"
  domain       = "dashboard.example.invalid"

  env_vars = [
    {
      key       = "AEX_API_BASE_URL"
      target    = ["production", "preview"]
      reference = "https://api.example.invalid"
    },
  ]
}

run "dev_and_prd_are_separate_projects" {
  command = plan

  assert {
    condition     = vercel_project.this.name == "aex-dashboard-dev"
    error_message = "The project name must carry the plane suffix."
  }

  assert {
    condition     = output.project_name == "aex-dashboard-dev"
    error_message = "The reported project name must carry the plane suffix."
  }
}

run "the_prd_plane_produces_a_different_project" {
  command = plan

  variables {
    plane = "prd"
  }

  assert {
    condition     = vercel_project.this.name == "aex-dashboard-prd"
    error_message = "The prd plane must produce its own project, never share the dev one."
  }
}

run "production_auto_assignment_is_off" {
  command = plan

  assert {
    condition     = vercel_project.this.auto_assign_custom_domains == false
    error_message = "Production domain auto-assignment must stay off; promotion is an explicit release step."
  }
}

run "nothing_is_built_by_the_project" {
  command = plan

  assert {
    condition     = vercel_project.this.build_command == null
    error_message = "No build command may be configured; the deployment is prebuilt."
  }

  assert {
    condition     = vercel_project.this.framework == null
    error_message = "No framework preset may be configured; a preset implies a build step."
  }

  assert {
    condition     = vercel_project.this.install_command == null
    error_message = "No install command may be configured."
  }
}

run "environment_values_are_references_and_marked_sensitive" {
  command = plan

  assert {
    condition = alltrue([
      for k, e in vercel_project_environment_variable.this : e.sensitive == true
    ])
    error_message = "Every environment variable must be marked sensitive."
  }

  assert {
    condition = alltrue([
      for k, e in vercel_project_environment_variable.this : can(regex("^(AEX|NEXT_PUBLIC)_", e.key))
    ])
    error_message = "Every environment key must be namespaced."
  }
}

run "rejects_auto_assigning_production_domains" {
  command = plan

  variables {
    auto_assign_domains = true
  }

  expect_failures = [var.auto_assign_domains]
}

run "rejects_a_build_command" {
  command = plan

  variables {
    build_command = "bun run build"
  }

  expect_failures = [var.build_command]
}

run "rejects_a_framework_preset" {
  command = plan

  variables {
    framework = "nextjs"
  }

  expect_failures = [var.framework]
}

run "rejects_a_plane_suffix_in_the_base_name" {
  command = plan

  variables {
    project_name = "aex-dashboard-prd"
  }

  expect_failures = [var.project_name]
}

run "rejects_an_environment_key_outside_the_namespace" {
  command = plan

  variables {
    env_vars = [
      {
        key       = "DATABASE_URL"
        target    = ["production"]
        reference = "postgres://localhost/aex"
      },
    ]
  }

  expect_failures = [var.env_vars]
}

run "rejects_an_environment_value_that_looks_like_a_credential" {
  command = plan

  variables {
    env_vars = [
      {
        key       = "AEX_PROVIDER_KEY"
        target    = ["production"]
        reference = "sk-not-a-real-key-000000000000"
      },
    ]
  }

  expect_failures = [var.env_vars]
}
