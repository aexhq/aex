# `cloudwatch-alarms`

Metric alarms with the metadata an operator needs at three in the morning: who
owns it, how urgent it is, where the runbook is, and whether anything automatic
will happen.

The category matters more than it looks. A **correctness** alarm says the
product computed the wrong answer; a **telemetry-loss** alarm says the product
stopped talking. Those demand opposite responses - one is a data incident, the
other might be a broken exporter - so they are tagged apart and never routed by
the same rule.

A telemetry-loss alarm must treat missing data as breaching. Silence is exactly
the condition it exists to catch, so an alarm that goes quiet when the metric
goes quiet is worse than no alarm at all.

## Inputs

| Name | Type | Description |
| --- | --- | --- |
| `alarm_specs` | `list(object)` | `{ name, description, owner, urgency, runbook_url, action_class, category, namespace, metric_name, statistic, period, evaluation_periods, datapoints_to_alarm, threshold, comparison_operator, treat_missing_data, dimensions }`. |
| `sns_topic_arn` | `string` | Topic every alarm action publishes to. |
| `tags` | `map(string)` | Tags merged with the per-alarm classification tags. |

## Outputs

| Name | Description |
| --- | --- |
| `alarm_arns` | Alarm name to alarm ARN. |
| `alarm_categories` | Alarm name to category, for routing rules. |

## Policy asserted

- Every alarm carries an owner from the closed stream list; an unknown owner is
  rejected.
- Every alarm carries an https runbook URL, both as a tag and inside the alarm
  description an operator sees first. An empty runbook is rejected.
- Correctness and telemetry-loss alarms carry different category tags, and a
  telemetry-loss alarm additionally notifies on insufficient data while a
  correctness alarm does not.
- A telemetry-loss alarm that treats missing data as anything but breaching is
  rejected.
- Urgency, action class, category, statistic, comparison operator and
  missing-data treatment all come from closed sets.

## Not asserted here

Whether the alarm ever fires depends on metrics a deployed product emits. The
emitting deployable's live suite owns that proof.
