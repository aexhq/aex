# `api/openapi/`

Domain-owned OpenAPI 3.1 fragments, one file per authority. They are inputs to
the canonical bundle, not the bundle itself.

Owner: the contracts stream.

Route delivery metadata is authored in
`api/schemas/registries/routes-meta.yaml`. `scenarioOwners` and
`operationOwners` are planned ownership: they say which immutable artifact is
responsible for closing a route. `servedOperations` is the narrower runtime
fact: it lists only operations a production composition actually mounts. The
generator emits both into `api/generated/registries/routes.json`, while neither
enters the public bundle or its digest. A planned owner is never mount proof.
