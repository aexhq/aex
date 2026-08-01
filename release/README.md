# `release/`

Composition manifest, artifact and evidence schemas plus the policy the release
tool enforces. It never contains a live secret.

Production promotes a complete immutable composition manifest; a one-service
release creates a new complete manifest whose only artifact change is that
service's digest.
