terraform {
  required_version = ">= 1.14.0"

  required_providers {
    vercel = {
      source  = "vercel/vercel"
      version = "~> 5.0"
    }
  }
}
