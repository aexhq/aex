# `infra/`

`modules/` holds the public reusable Terraform modules the product ships.
`examples/` holds sanitized self-host and localhost roots.

Private environment roots live in the private `platform` repository and consume
an exact module version, never this directory's `main`.
