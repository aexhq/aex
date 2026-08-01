# `sns-ops-topic`

The operational notification topic alarms publish to.

An operational topic feeds a pager. An open publish policy is therefore an open
pager, which is why every publisher is named explicitly and a wildcard - even a
partial one such as `role/*` - is rejected.

The topic ARN is built from values the root supplies. The module does not look
up the account it is running in.

## Inputs

| Name | Type | Description |
| --- | --- | --- |
| `name` | `string` | Topic name, `aex-<...>`. |
| `region` | `string` | Region, for the topic ARN. |
| `account_id` | `string` | Account, for the topic ARN. Supplied by the root. |
| `partition` | `string` | AWS partition. |
| `publish_principals` | `list(object)` | `{ type, identifier }`; explicit publishers only. |
| `subscriptions` | `list(object)` | `{ protocol, endpoint }`. |
| `kms_key_arn` | `string` | Optional customer-managed key. |
| `tags` | `map(string)` | Tags. |

## Outputs

| Name | Description |
| --- | --- |
| `topic_arn` | ARN of the topic. |
| `subscription_arns` | Subscription key to subscription ARN. |

## Policy asserted

- No publish principal is a wildcard, whole or partial; an empty principal list
  is also rejected.
- Every policy statement grants `sns:Publish` and nothing else, scoped to this
  topic's ARN.
- An `https` subscription endpoint must actually be an `https` URL.
- Subscription protocols come from a closed set.

## Not asserted here

Whether a notification is delivered is an operator-facing path with no product
deployable behind it, so no live suite owns it.
