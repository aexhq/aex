# `crates/`

Library crates. Every crate is `publish = false` public Apache-2.0 source, is
named `aex-<authority>`, and lives in a directory of exactly that name.

Dependencies point inward: contract -> domain -> application ports, with
adapters and deployables pointing toward them. A `*-domain` crate may not depend
on an AWS SDK, an HTTP framework, a Tokio global or a provider client.
`tools/aex-workspace-check` enforces this.
