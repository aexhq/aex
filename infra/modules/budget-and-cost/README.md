# `budget-and-cost`

Account budgets and cost anomaly detection.

The mandatory tag set is the point of this module. Without `plane`, `region`,
`releaseId` and `deployable` on every resource, a cost line cannot be attributed
to anything, and a cost anomaly becomes an unanswerable question. The budgets
filter on all four, so an alert names something someone can act on.

`releaseId` is in the set deliberately: it is what makes "this release costs 30
percent more than the last one" a question the billing data can answer.

## Inputs

| Name | Type | Description |
| --- | --- | --- |
| `required_tags` | `list(string)` | Must include `plane`, `region`, `releaseId`, `deployable`. |
| `budgets` | `list(object)` | `{ name, limit_amount, limit_unit, time_unit, threshold_percent, cost_filter_tags }`. |
| `anomaly_thresholds` | `object` | `{ absolute_usd, percentage }`. |
| `sns_topic_arn` | `string` | Notification target. |
| `tags` | `map(string)` | Tags on the anomaly monitor and subscription. |

## Outputs

| Name | Description |
| --- | --- |
| `budget_arns` | Budget name to budget ARN. |
| `anomaly_monitor_arn` | ARN of the cost anomaly monitor. |
| `required_tags` | The mandatory tag set, re-exported for callers to enforce. |

## Policy asserted

- The mandatory tag set includes `plane`, `region`, `releaseId` and
  `deployable`; a set missing any of them is rejected.
- Every budget filters on the full mandatory tag set; a budget filtering on a
  subset is rejected.
- Every budget notifies on both actual and forecasted spend, and every
  notification publishes to the ops topic.
- Anomaly detection fires on either the absolute or the percentage threshold,
  and both thresholds must be positive.
- Budget limits must be positive, denominated in USD, and use a valid time unit.

## Not asserted here

Budget and anomaly evaluation is an account-level billing behaviour with no
deployable runtime path to probe.
