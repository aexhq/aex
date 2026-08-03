# `github-oidc-role`

One role a GitHub Actions job may assume through OIDC. No long-lived access key
exists anywhere in this design.

A role carries exactly one permission profile. The `publish` profile mints
bytes; the `deploy` profile rolls them out. Keeping them disjoint means a single
compromised job cannot both forge an artifact and put it in front of traffic.

## Inputs

| Name | Type | Description |
| --- | --- | --- |
| `role_name` | `string` | Exact plane-scoped `aex-...` physical name; required so shared-account planes cannot collide. |
| `repository` | `string` | Exact `owner/name`; no wildcard. |
| `oidc_provider_arn` | `string` | The account's GitHub OIDC provider. |
| `allowed_refs` | `list(string)` | Exact full refs, e.g. `refs/heads/main`. |
| `allowed_environments` | `list(string)` | Optional exact protected `aex-dev`/`aex-prd` environment. When set, ref-only subjects are excluded. |
| `allowed_workflows` | `list(string)` | Exact workflow paths under `.github/workflows/`. |
| `permissions_profile` | `string` | `publish`, `plan`, `deploy` or `readonly`. |
| `permission_profiles` | `map(list(string))` | Actions per profile, supplied by the root. |
| `profile_resources` | `map(list(string))` | Resources per profile. |
| `max_session_duration` | `number` | Session duration in seconds. |
| `tags` | `map(string)` | Tags. |

## Outputs

| Name | Description |
| --- | --- |
| `role_arn` | ARN of the role a workflow job assumes. |
| `role_name` | Physical role name. |
| `allowed_subjects` | Exact OIDC subjects permitted to assume the role. |

## Policy asserted

- The trust condition names the exact repository and the exact ref through
  `sub`, and the exact workflow file through `job_workflow_ref`. A new workflow
  in the same repository on the same branch cannot assume the role.
- No trust condition value contains a wildcard; a wildcard repository, a ref
  pattern and a workflow glob are all rejected.
- The audience is pinned to `sts.amazonaws.com`.
- A protected-environment role accepts only the environment-shaped GitHub
  subject. It does not also accept the branch subject, because doing so would
  let the same reusable workflow bypass the Environment approval gate.
- `publish` and `deploy` are disjoint: a profile map where an action appears in
  both is rejected, and the rendered policy of a `publish` role shares no action
  with `deploy`.
- The role grants only the actions of its own profile.
- No profile may grant a wildcard action.
- The physical name is supplied explicitly and must use an exact lowercase
  `aex-...` namespace. The module never derives the same role name for two
  planes that share one account.

## Not asserted here

Whether GitHub issues the token this trust policy expects is proven when a
workflow assumes the role. There is no deployed product surface to probe.
