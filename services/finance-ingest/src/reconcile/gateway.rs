//! The reconciler's direct, idempotent path to Stripe's official HTTPS API.

use aex_payment_contracts::{
    PaymentCommand, PaymentCommandEnvelope, PaymentFailure, PaymentFailureClass, PaymentResult,
    ProviderObjectRef, TaxMode, UnknownEvidence,
};
use aex_wire::types::{Cents, HttpsUrl, Timestamp};
use zeroize::Zeroizing;

use super::sweep::SweepError;

const STRIPE_API: &str = "https://api.stripe.com/v1";

/// Executes a previously admitted command or a read-only outcome lookup.
#[async_trait::async_trait]
pub trait EffectRecoveryGateway: Send + Sync + 'static {
    /// Executes the exact finance-authored command with its stable idempotency key.
    async fn execute(&self, envelope: &PaymentCommandEnvelope)
    -> Result<PaymentResult, SweepError>;
}

/// Redacted Stripe credential resolved from Secrets Manager at process start.
pub struct StripeSecret(Zeroizing<String>);

impl StripeSecret {
    /// Wraps a non-empty provider credential.
    pub fn new(secret: String) -> Result<Self, SweepError> {
        if secret.is_empty() {
            return Err(SweepError::RecoveryUnknown(
                "Stripe secret is empty".to_owned(),
            ));
        }
        Ok(Self(Zeroizing::new(secret)))
    }
}

impl core::fmt::Debug for StripeSecret {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("StripeSecret(<redacted>)")
    }
}

/// Direct Stripe adapter. Redirects and client-level retries are disabled so
/// finance's durable effect state remains the only retry authority.
#[derive(Debug, Clone)]
pub struct StripeEffectRecoveryGateway {
    client: reqwest::Client,
    secret: std::sync::Arc<StripeSecret>,
}

impl StripeEffectRecoveryGateway {
    /// Builds the pinned provider client.
    pub fn new(secret: StripeSecret) -> Result<Self, SweepError> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(core::time::Duration::from_secs(8))
            .build()
            .map_err(|error| SweepError::RecoveryUnknown(error.to_string()))?;
        Ok(Self {
            client,
            secret: std::sync::Arc::new(secret),
        })
    }

    async fn post(
        &self,
        path: &str,
        envelope: &PaymentCommandEnvelope,
        form: Vec<(String, String)>,
    ) -> Result<PaymentResult, SweepError> {
        let effect = envelope.command.effect();
        let response = self
            .client
            .post(format!("{STRIPE_API}/{path}"))
            .bearer_auth(self.secret.0.as_str())
            .header("Stripe-Version", "2025-03-31.basil")
            .header("Idempotency-Key", &envelope.provider_idempotency_key)
            .form(&form)
            .send()
            .await;
        let response = match response {
            Ok(response) => response,
            Err(error) if error.is_timeout() => {
                return Ok(PaymentResult::Unknown {
                    effect,
                    evidence: UnknownEvidence::Timeout { waited_ms: 8_000 },
                });
            }
            Err(_) => {
                return Ok(PaymentResult::Unknown {
                    effect,
                    evidence: UnknownEvidence::TransportLost,
                });
            }
        };
        let status = response.status();
        let request_id = response
            .headers()
            .get("request-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|error| SweepError::Decode(error.to_string()))?;
        if status.is_success() {
            return success(envelope, &body);
        }
        if status.as_u16() == 400 || status.as_u16() == 402 {
            let code = body
                .pointer("/error/code")
                .and_then(serde_json::Value::as_str)
                .and_then(|value| aex_wire::types::StableCode::parse(value).ok());
            let class = if body
                .pointer("/error/decline_code")
                .and_then(serde_json::Value::as_str)
                .is_some()
            {
                PaymentFailureClass::CardDeclined
            } else {
                PaymentFailureClass::InvalidRequest
            };
            return Ok(PaymentResult::from_failure(
                effect,
                PaymentFailure {
                    class,
                    provider_code: code,
                    decline_code: body
                        .pointer("/error/decline_code")
                        .and_then(serde_json::Value::as_str)
                        .and_then(|value| aex_wire::types::StableCode::parse(value).ok()),
                    retryable: false,
                },
            ));
        }
        let evidence = if status.is_server_error() {
            UnknownEvidence::ServerError {
                status: status.as_u16(),
            }
        } else {
            UnknownEvidence::AmbiguousResponse {
                provider_code: request_id
                    .as_deref()
                    .and_then(|value| aex_wire::types::StableCode::parse(value).ok()),
            }
        };
        Ok(PaymentResult::Unknown { effect, evidence })
    }
}

#[async_trait::async_trait]
impl EffectRecoveryGateway for StripeEffectRecoveryGateway {
    async fn execute(
        &self,
        envelope: &PaymentCommandEnvelope,
    ) -> Result<PaymentResult, SweepError> {
        if envelope.deadline.unix_millis()
            <= time::OffsetDateTime::now_utc().unix_timestamp() * 1_000
        {
            return Ok(PaymentResult::Unknown {
                effect: envelope.command.effect(),
                evidence: UnknownEvidence::Timeout { waited_ms: 0 },
            });
        }
        let metadata = |form: &mut Vec<(String, String)>| {
            form.extend([
                (
                    "metadata[aex_effect_id]".to_owned(),
                    envelope.metadata.effect.0.to_string(),
                ),
                (
                    "metadata[aex_org_id]".to_owned(),
                    envelope.metadata.organization.to_string(),
                ),
                (
                    "metadata[aex_command_kind]".to_owned(),
                    envelope.metadata.kind.as_str().to_owned(),
                ),
                (
                    "metadata[aex_credit_cents]".to_owned(),
                    envelope.metadata.credit.get().to_string(),
                ),
            ]);
        };
        match &envelope.command {
            PaymentCommand::EnsureCustomer { email, .. } => {
                let mut form = vec![("email".to_owned(), email.expose().to_owned())];
                metadata(&mut form);
                self.post("customers", envelope, form).await
            }
            PaymentCommand::CreateTopUpCheckout {
                amount,
                success_url,
                cancel_url,
                customer,
                tax,
                ..
            } => {
                let mut form = vec![
                    ("mode".to_owned(), "payment".to_owned()),
                    ("customer".to_owned(), customer.0.clone()),
                    ("success_url".to_owned(), success_url.as_str().to_owned()),
                    ("cancel_url".to_owned(), cancel_url.as_str().to_owned()),
                    ("line_items[0][quantity]".to_owned(), "1".to_owned()),
                    (
                        "line_items[0][price_data][currency]".to_owned(),
                        "usd".to_owned(),
                    ),
                    (
                        "line_items[0][price_data][unit_amount]".to_owned(),
                        amount.get().to_string(),
                    ),
                    (
                        "line_items[0][price_data][product_data][name]".to_owned(),
                        "AEX prepaid credit".to_owned(),
                    ),
                    (
                        "automatic_tax[enabled]".to_owned(),
                        matches!(tax, TaxMode::ProviderAutomatic).to_string(),
                    ),
                ];
                metadata(&mut form);
                self.post("checkout/sessions", envelope, form).await
            }
            PaymentCommand::CreatePaymentMethodSession {
                success_url,
                cancel_url,
                customer,
                ..
            } => {
                let mut form = vec![
                    ("mode".to_owned(), "setup".to_owned()),
                    ("customer".to_owned(), customer.0.clone()),
                    ("success_url".to_owned(), success_url.as_str().to_owned()),
                    ("cancel_url".to_owned(), cancel_url.as_str().to_owned()),
                    ("payment_method_types[0]".to_owned(), "card".to_owned()),
                ];
                metadata(&mut form);
                self.post("checkout/sessions", envelope, form).await
            }
            PaymentCommand::DetachPaymentMethod { method, .. } => {
                self.post(
                    &format!("payment_methods/{}/detach", method.0),
                    envelope,
                    Vec::new(),
                )
                .await
            }
            PaymentCommand::RefundCharge {
                original, amount, ..
            } => {
                let mut form = vec![
                    ("charge".to_owned(), original.0.clone()),
                    ("amount".to_owned(), amount.get().to_string()),
                ];
                metadata(&mut form);
                self.post("refunds", envelope, form).await
            }
            PaymentCommand::LookupEffectOutcome { effect, .. } => Ok(PaymentResult::Unknown {
                effect: *effect,
                evidence: UnknownEvidence::AmbiguousResponse {
                    provider_code: aex_wire::types::StableCode::parse("reconciliation_required")
                        .ok(),
                },
            }),
        }
    }
}

fn success(
    envelope: &PaymentCommandEnvelope,
    body: &serde_json::Value,
) -> Result<PaymentResult, SweepError> {
    let provider_ref = body
        .get("id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| SweepError::Decode("Stripe success omitted object id".to_owned()))?;
    let created = body
        .get("created")
        .and_then(serde_json::Value::as_i64)
        .ok_or_else(|| SweepError::Decode("Stripe success omitted created time".to_owned()))?;
    let hosted = body
        .get("url")
        .and_then(serde_json::Value::as_str)
        .map(|url| {
            let expires = body
                .get("expires_at")
                .and_then(serde_json::Value::as_i64)
                .ok_or_else(|| SweepError::Decode("Stripe session omitted expiry".to_owned()))?;
            Ok(aex_payment_contracts::HostedSession {
                url: HttpsUrl::parse(url).map_err(|error| SweepError::Decode(error.to_string()))?,
                expires_at: Timestamp::from_unix_millis(expires.saturating_mul(1_000))
                    .map_err(|error| SweepError::Decode(error.to_string()))?,
            })
        })
        .transpose()?;
    let charged = match &envelope.command {
        PaymentCommand::CreateTopUpCheckout { amount, .. }
        | PaymentCommand::RefundCharge { amount, .. } => *amount,
        _ => Cents::ZERO,
    };
    Ok(PaymentResult::Succeeded {
        effect: envelope.command.effect(),
        provider_ref: ProviderObjectRef(provider_ref.to_owned()),
        provider_created_at: Timestamp::from_unix_millis(created.saturating_mul(1_000))
            .map_err(|error| SweepError::Decode(error.to_string()))?,
        hosted,
        charged,
        tax: None,
    })
}

#[cfg(test)]
mod tests {
    use super::StripeSecret;

    #[test]
    fn a_stripe_secret_never_renders() {
        let secret = StripeSecret::new("sk_test_super_secret".to_owned()).expect("secret");
        let rendered = format!("{secret:?}");
        assert!(!rendered.contains("sk_test"));
        assert_eq!(rendered, "StripeSecret(<redacted>)");
    }
}
