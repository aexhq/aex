# `eventbridge-scheduler`

A schedule group and the schedules in it: the content lifecycle sweep, the
observation export launch, the storage accrual cron.

Every target is immutable. A Lambda target is a qualified alias; an ECS target
is a task definition pinned to a revision. A schedule that names a mutable
function or an unrevisioned family would quietly start running different code
than the release manifest says it does, and nothing in the deployment record
would show it.

## Inputs

| Name | Type | Description |
| --- | --- | --- |
| `group_name` | `string` | Schedule group, `aex-<...>`. |
| `schedules` | `list(object)` | `{ name, description, expression, expression_timezone, flexible_window_minutes, target_arn, role_arn, input, dead_letter_arn, maximum_retry_attempts, maximum_event_age_in_sec }`. |
| `kms_key_arn` | `string` | Optional customer-managed key for payload encryption. |

## Outputs

| Name | Description |
| --- | --- |
| `schedule_arns` | Schedule name to schedule ARN. |
| `group_name` | Name of the schedule group. |

## Policy asserted

- Every target is a qualified Lambda alias ARN or an ECS task definition ARN
  pinned to a revision. An unqualified function, an unrevisioned task family and
  a `$LATEST` qualifier are each rejected.
- Every expression is `rate(...)`, `cron(...)` or `at(...)`.
- Every input parses as JSON.
- Every schedule names an IAM role ARN to invoke its target with.
- Schedule names are unique, and every schedule lands in the configured group
  and is enabled.
- A zero flexible window switches the mode to `OFF` rather than requesting a
  zero-minute window.

## Not asserted here

Whether a schedule fires and whether the target does the right thing belongs to
the live suite of the deployable behind each target.
