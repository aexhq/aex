use brain_protocol::Event;

use crate::{
    Error, Result,
    brain::BrainClient,
    store::{Db, ModelUsageRow, SessionRow},
};

pub async fn reconcile_session(
    db: &Db,
    brain: &BrainClient,
    session: &SessionRow,
) -> Result<ModelUsageRow> {
    let mut usage = db.model_usage(session.id.clone()).await?;
    loop {
        let after = u64::try_from(usage.folded_sequence)
            .map_err(|_| Error::Internal("negative model-usage cursor".into()))?;
        let page = brain.events(&session.id, after).await?;
        if page.events.is_empty() {
            if page.next_cursor != after {
                return Err(Error::Upstream(
                    "Brain advanced an empty event page cursor".into(),
                ));
            }
            break;
        }
        fold_events(&mut usage, &page.events)?;
        if page.next_cursor
            != u64::try_from(usage.folded_sequence)
                .map_err(|_| Error::Internal("negative model-usage cursor".into()))?
        {
            return Err(Error::Upstream(
                "Brain event page cursor disagrees with its final event".into(),
            ));
        }
    }
    usage.updated_ms = crate::now_ms();
    db.apply_model_usage(
        session.id.clone(),
        session.account_id.clone(),
        usage.clone(),
    )
    .await?;
    Ok(usage)
}

pub async fn reconcile_account(
    db: &Db,
    brain: &BrainClient,
    account_id: &str,
) -> Result<Vec<(SessionRow, ModelUsageRow)>> {
    let mut reconciled = Vec::new();
    for session in db.sessions_of(account_id.to_owned()).await? {
        let usage = reconcile_session(db, brain, &session).await?;
        reconciled.push((session, usage));
    }
    Ok(reconciled)
}

fn fold_events(usage: &mut ModelUsageRow, events: &[Event]) -> Result<()> {
    for event in events {
        let sequence = i64::try_from(event.sequence).map_err(|_| {
            Error::Upstream("Brain event sequence exceeds the billing range".into())
        })?;
        if sequence <= usage.folded_sequence {
            continue;
        }
        if event.event_type == "model_call_ended" {
            let receipt = &event.data["result"]["usage"];
            let provider_cost = receipt.get("provider_cost_usd").ok_or_else(|| {
                Error::Upstream("model result has no provider cost receipt".into())
            })?;
            let cost = provider_cost.as_str().ok_or_else(|| {
                Error::Upstream("model result has an invalid provider cost receipt".into())
            })?;
            let gateway_cost_nano_usd = usage
                .gateway_cost_nano_usd
                .checked_add(decimal_usd_to_nano(cost)?)
                .ok_or_else(|| Error::Internal("model cost exceeds the billing range".into()))?;
            let model_calls = usage.model_calls.checked_add(1).ok_or_else(|| {
                Error::Internal("model-call count exceeds the billing range".into())
            })?;
            let input_tokens = checked_counter(
                usage.input_tokens,
                receipt.get("input_tokens"),
                "input token",
            )?;
            let output_tokens = checked_counter(
                usage.output_tokens,
                receipt.get("output_tokens"),
                "output token",
            )?;
            usage.gateway_cost_nano_usd = gateway_cost_nano_usd;
            usage.model_calls = model_calls;
            usage.input_tokens = input_tokens;
            usage.output_tokens = output_tokens;
        }
        usage.folded_sequence = sequence;
    }
    Ok(())
}

fn checked_counter(current: i64, value: Option<&serde_json::Value>, label: &str) -> Result<i64> {
    let value = value
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| Error::Upstream(format!("model result has no valid {label} count")))?;
    current
        .checked_add(
            i64::try_from(value)
                .map_err(|_| Error::Upstream(format!("model {label} count is too large")))?,
        )
        .ok_or_else(|| Error::Internal(format!("model {label} count exceeds the billing range")))
}

fn decimal_usd_to_nano(value: &str) -> Result<i64> {
    let value = value.trim();
    if value.is_empty() || value.starts_with('-') || value.starts_with('+') {
        return Err(Error::Upstream(
            "provider cost is not a non-negative decimal".into(),
        ));
    }
    let (coefficient, exponent) =
        value
            .split_once(['e', 'E'])
            .map_or(Ok((value, 0i32)), |(coefficient, exponent)| {
                let exponent = exponent
                    .parse::<i32>()
                    .map_err(|_| Error::Upstream("provider cost exponent is invalid".into()))?;
                Ok((coefficient, exponent))
            })?;
    let (whole, fraction) = coefficient.split_once('.').unwrap_or((coefficient, ""));
    if whole.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(Error::Upstream("provider cost is not a decimal".into()));
    }
    let digits = format!("{whole}{fraction}");
    let unscaled = digits
        .parse::<i128>()
        .map_err(|_| Error::Upstream("provider cost exceeds the billing range".into()))?;
    let scale = exponent
        .checked_sub(i32::try_from(fraction.len()).map_err(|_| {
            Error::Upstream("provider cost precision exceeds the billing range".into())
        })?)
        .and_then(|scale| scale.checked_add(9))
        .ok_or_else(|| Error::Upstream("provider cost scale is invalid".into()))?;
    let nano = if scale >= 0 {
        unscaled
            .checked_mul(power_of_ten(scale as u32))
            .ok_or_else(|| Error::Upstream("provider cost exceeds the billing range".into()))?
    } else {
        unscaled / power_of_ten(scale.unsigned_abs())
    };
    i64::try_from(nano)
        .map_err(|_| Error::Upstream("provider cost exceeds the billing range".into()))
}

fn power_of_ten(exponent: u32) -> i128 {
    10i128.checked_pow(exponent).unwrap_or(i128::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use brain_protocol::EventId;
    use serde_json::json;

    #[test]
    fn provider_decimal_costs_convert_without_float_rounding() {
        assert_eq!(decimal_usd_to_nano("0.00012925").unwrap(), 129_250);
        assert_eq!(decimal_usd_to_nano("2.925E-05").unwrap(), 29_250);
        assert_eq!(decimal_usd_to_nano("1").unwrap(), 1_000_000_000);
        assert_eq!(decimal_usd_to_nano("0.0000000009").unwrap(), 0);
        assert!(decimal_usd_to_nano("-1").is_err());
    }

    #[test]
    fn model_receipts_fold_once_and_malformed_events_do_not_partially_mutate_usage() {
        let event = Event {
            event_id: EventId::new("evt_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            sequence: 2,
            recorded_at_ms: 10,
            event_type: "model_call_ended".into(),
            data: json!({"result":{"usage":{
                "provider_cost_usd":"0.00012925",
                "input_tokens":12,
                "output_tokens":3
            }}}),
        };
        let mut usage = ModelUsageRow::default();
        fold_events(&mut usage, std::slice::from_ref(&event)).unwrap();
        fold_events(&mut usage, &[event]).unwrap();
        assert_eq!(usage.folded_sequence, 2);
        assert_eq!(usage.gateway_cost_nano_usd, 129_250);
        assert_eq!(usage.model_calls, 1);
        assert_eq!((usage.input_tokens, usage.output_tokens), (12, 3));

        let before = usage.clone();
        let malformed = Event {
            event_id: EventId::new("evt_bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
            sequence: 3,
            recorded_at_ms: 11,
            event_type: "model_call_ended".into(),
            data: json!({"result":{"usage":{
                "provider_cost_usd":1,
                "input_tokens":1,
                "output_tokens":1
            }}}),
        };
        assert!(fold_events(&mut usage, &[malformed]).is_err());
        assert_eq!(usage, before);
    }
}
