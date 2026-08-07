# `iam-deployable-role`

One execution role for one deployable. Roles are never shared between
deployables, because a shared role makes the negative half of the permission
matrix untestable.

The grants normally come from the generated IAM action lists in the regional
bundle. The module does not read that bundle; the root decodes it and passes the
grants in.

## Inputs

| Name | Type | Description |
| --- | --- | --- |
| `deployable` | `string` | Deployable id as named in `release/units.toml`. |
| `plane` | `string` | `dev` or `prd`. |
| `region` | `string` | AWS region code. |
| `assume_principal` | `object` | `{ type, identifiers }`; who may assume the role. |
| `action_grants` | `list(object)` | `{ sid, actions, resources, scopable, condition_operator, condition_key, condition_values }`; the operator defaults to `StringEquals`, and only reviewed operators are accepted. |
| `wildcard_resource_allowlist` | `list(string)` | Exhaustive list of actions permitted on `Resource: "*"`. |
| `boundary_policy_arn` | `string` | Optional permissions boundary. |
| `max_session_duration` | `number` | Session duration in seconds. |
| `tags` | `map(string)` | Tags applied to the role. |

The role name is `aex-<plane>-<deployable>`.

## Outputs

| Name | Description |
| --- | --- |
| `role_arn` | ARN of the deployable role. |
| `role_name` | Physical role name. |

## Policy asserted

- No statement grants `Action: "*"`, and no statement grants a whole-service
  wildcard such as `dynamodb:*`. Every action must be `service:Action`.
- `Resource: "*"` is permitted only for actions in `wildcard_resource_allowlist`,
  which is capped at twelve entries and may not itself be a wildcard.
- Every grant marked `scopable` must carry a plane or region condition; a
  scopable grant with no condition is rejected.
- No two grants share a statement id. AWS rejects a policy document with a
  duplicate `Sid`, so a generator that emits one — a table read and a stream read
  on the same table, for instance — fails here rather than at apply.
- The trust policy names explicit principals, never a wildcard, and grants only
  `sts:AssumeRole`.

## Not asserted here

Whether AWS actually denies everything outside the grant is the `aws.iam.denial`
seam, owned by the deployable's own live suite. A plan proves the document; only
a live call proves the denial.
