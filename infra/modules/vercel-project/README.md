# `vercel-project`

One Vercel project for one plane, plus its domain and runtime environment
variables.

Nothing is built here. The artifact is a prebuilt Build Output API tree whose
digest is already recorded in the release manifest, and it is deployed with
`vercel deploy --prebuilt`. A build command or a framework preset would produce
bytes nothing has attested, so both are rejected rather than merely left unset.

Production domain auto-assignment is off. Promotion is a decision recorded in a
manifest, not a side effect of the newest deployment appearing.

`dev` and `prd` are always two projects. The module appends the plane to the
name, and a base name that already ends in a plane suffix is rejected so the two
cannot collide on one project.

Provider: `vercel/vercel ~> 5.0`.

## Inputs

| Name | Type | Description |
| --- | --- | --- |
| `team_id` | `string` | Vercel team, from the environment binding. |
| `project_name` | `string` | Base name; the module appends the plane. |
| `plane` | `string` | `dev` or `prd`. |
| `domain` | `string` | Domain attached to the project. |
| `auto_assign_domains` | `bool` | Must stay `false`. |
| `build_command` | `string` | Must stay `null`. |
| `framework` | `string` | Must stay `null`. |
| `env_vars` | `list(object)` | `{ key, target, reference, comment }`; values are reference handles. |
| `serverless_function_region` | `string` | Vercel region code. |

## Outputs

| Name | Description |
| --- | --- |
| `project_id` | Project id; the binding records it per plane. |
| `project_name` | Physical project name including the plane suffix. |
| `domain` | Domain attached to the project. |

## Policy asserted

- Production auto-assignment is off; enabling it is rejected.
- No build command, no framework preset, no install command. Setting either of
  the first two is rejected.
- `dev` and `prd` produce different project names, and a base name that already
  carries a plane suffix is rejected.
- Every environment key is namespaced `AEX_*` or `NEXT_PUBLIC_*`, every value is
  a reference handle, and a value that looks like a credential is rejected.
- Every environment variable is marked sensitive.
- System environment variables are not automatically exposed.

## Not asserted here

Deployment identity and plane binding are proven by the dashboard and site live
suites that deploy into the project.
