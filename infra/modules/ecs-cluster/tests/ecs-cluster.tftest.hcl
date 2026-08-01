mock_provider "aws" {}

variables {
  name = "aex-dev-eu-west-1-regional"
}

run "container_insights_is_on_by_default" {
  command = plan

  assert {
    condition     = one(aws_ecs_cluster.this.setting).name == "containerInsights"
    error_message = "The cluster must declare the containerInsights setting."
  }

  assert {
    condition     = one(aws_ecs_cluster.this.setting).value == "enhanced"
    error_message = "Container Insights must default to enhanced."
  }

  assert {
    condition     = aws_ecs_cluster.this.name == var.name
    error_message = "The cluster must be created under the name the root composed."
  }
}

run "rejects_disabled_container_insights" {
  command = plan

  variables {
    container_insights = "disabled"
  }

  expect_failures = [var.container_insights]
}

run "rejects_a_cluster_outside_the_aex_namespace" {
  command = plan

  variables {
    name = "regional"
  }

  expect_failures = [var.name]
}
