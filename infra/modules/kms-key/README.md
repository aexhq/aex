# `kms-key`

A customer-managed KMS key plus its alias. The alias is the stable handle an
environment binding names; the key id never appears in a binding.

The module invents no policy statement. The root supplies the complete key
policy, including the account administration grant, so the set of principals
that can use the key is auditable in one place.

## Inputs

| Name | Type | Description |
| --- | --- | --- |
| `alias` | `string` | Alias including the `alias/` prefix; must be `alias/aex-<name>`. |
| `description` | `string` | Purpose of the key, recorded on the key. |
| `rotation` | `bool` | Automatic key-material rotation. Must be `true`. |
| `rotation_period_days` | `number` | Rotation period, 90-2560 days. |
| `deletion_window_days` | `number` | Pending-deletion window; floor of 30 days. |
| `encryption_context_equals` | `map(string)` | Extra encryption-context pairs every data-plane grant must match. `aex:workspace` is per statement and is rejected here. |
| `policy_statements` | `list(object)` | The complete key policy. `data_plane = true` marks a grant used by product code over customer data. |
| `tags` | `map(string)` | Tags applied to the key. |

Each `policy_statements` element is `{ sid, effect, principal_type, principals,
actions, resources, data_plane, encryption_context_workspace }`. The workspace
value is normally the IAM policy variable `aws:PrincipalTag/aex:workspace`,
written with a doubled dollar sign in HCL so Terraform does not interpolate it.

## Outputs

| Name | Description |
| --- | --- |
| `key_arn` | ARN of the customer-managed key. |
| `alias_arn` | ARN of the alias. |

## Policy asserted

- Automatic key rotation is on; `rotation = false` is rejected by validation.
- The pending-deletion window is at least 30 days; a shorter window is rejected.
- Every data-plane grant carries a `kms:EncryptionContext:aex:workspace`
  condition, and every extra pair in `encryption_context_equals` is rendered
  onto those same grants.
- No statement grants `kms:*` on `Resource: "*"`, and no statement grants
  `Action: "*"`.
- No statement uses a wildcard principal.

## Not asserted here

Whether AWS actually denies a decrypt under a mismatched encryption context is
the `aws.kms.encryption_context` seam. No plan can prove it; only the live suite
that owns the seam can.
