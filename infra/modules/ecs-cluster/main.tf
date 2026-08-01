resource "aws_ecs_cluster" "this" {
  name = var.name
  tags = var.tags

  setting {
    name  = "containerInsights"
    value = var.container_insights
  }
}
