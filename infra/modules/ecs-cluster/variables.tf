variable "name" {
  type        = string
  description = "Physical cluster name. It is composed by the root from the plane and the region, exactly like every other physical name in a plane."

  validation {
    condition     = can(regex("^aex-[a-z0-9][a-z0-9-]{2,254}$", var.name))
    error_message = "The cluster name must start with `aex-` and be lowercase and hyphen-separated."
  }
}

variable "container_insights" {
  type        = string
  default     = "enhanced"
  description = "Container Insights level. `enhanced` is the level that reports per-task CPU, memory and network without an agent in the task."

  validation {
    condition     = contains(["enhanced", "enabled"], var.container_insights)
    error_message = "Container Insights must be `enhanced` or `enabled`. A cluster whose tasks report nothing is a cluster nobody can operate, so `disabled` is not an allowed configuration."
  }
}

variable "tags" {
  type        = map(string)
  default     = {}
  description = "Tags applied to the cluster."
}
