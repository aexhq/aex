//! The payments seam: how a top-up gets paid.
//!
//! Same adapter posture as the brain's substrate seams. `Stripe` is the production
//! implementation (Checkout Session per top-up; the customer pays in the browser). `Fake` is
//! the zero-config local default — every checkout is instantly "paid", no money moves, and the
//! server banners that loudly. Crediting is not done here: the caller marks the topup paid in
//! the store, idempotently, when `check` reports Paid — so webhook delivery and polling can
//! both drive it, in any order, any number of times.

use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::{Error, Result};

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaymentStatus {
    Pending,
    Paid,
    Expired,
}

pub struct Checkout {
    /// The provider's id for this payment (Stripe: the Checkout Session id).
    pub provider_ref: String,
    /// Where the customer pays.
    pub url: String,
}

/// A verified Stripe event that can change a top-up. Unknown event types verify successfully
/// and produce no action, as Stripe recommends for endpoints subscribed to broader event sets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StripeWebhookAction {
    Paid {
        topup_id: String,
        provider_ref: String,
        amount_cents: i64,
    },
    Expired {
        topup_id: String,
        provider_ref: String,
    },
}

/// Stripe endpoint-signing secret. Debug output is deliberately redacted.
#[derive(Clone)]
pub struct StripeWebhook {
    secret: std::sync::Arc<str>,
}

impl std::fmt::Debug for StripeWebhook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StripeWebhook")
            .field("secret", &"[REDACTED]")
            .finish()
    }
}

impl StripeWebhook {
    pub const TOLERANCE_SECONDS: i64 = 300;

    pub fn new(secret: String) -> Self {
        Self {
            secret: secret.into(),
        }
    }

    /// Verify Stripe's `t=...,v1=...` signature over the exact raw body, enforce the same
    /// five-minute replay window as Stripe's official libraries, then shape relevant events.
    pub fn verify(
        &self,
        signature_header: &str,
        body: &[u8],
        now_seconds: i64,
    ) -> Result<Option<StripeWebhookAction>> {
        let mut timestamp = None;
        let mut signatures = Vec::new();
        for part in signature_header.split(',') {
            let Some((name, value)) = part.trim().split_once('=') else {
                continue;
            };
            match name {
                "t" => {
                    timestamp = value.parse::<i64>().ok();
                }
                "v1" => {
                    if let Ok(bytes) = hex::decode(value) {
                        signatures.push(bytes);
                    }
                }
                _ => {}
            }
        }
        let timestamp = timestamp.ok_or_else(invalid_webhook)?;
        if now_seconds.abs_diff(timestamp) > Self::TOLERANCE_SECONDS as u64 {
            return Err(invalid_webhook());
        }

        let mut signed = timestamp.to_string().into_bytes();
        signed.push(b'.');
        signed.extend_from_slice(body);
        let valid = signatures.iter().any(|signature| {
            HmacSha256::new_from_slice(self.secret.as_bytes()).is_ok_and(|mut mac| {
                mac.update(&signed);
                mac.verify_slice(signature).is_ok()
            })
        });
        if !valid {
            return Err(invalid_webhook());
        }

        let event: serde_json::Value = serde_json::from_slice(body)
            .map_err(|_| Error::Invalid("invalid stripe webhook payload".into()))?;
        let kind = event["type"]
            .as_str()
            .ok_or_else(|| Error::Invalid("invalid stripe webhook payload".into()))?;
        if !matches!(
            kind,
            "checkout.session.completed"
                | "checkout.session.async_payment_succeeded"
                | "checkout.session.expired"
        ) {
            return Ok(None);
        }
        let object = &event["data"]["object"];
        let provider_ref = object["id"]
            .as_str()
            .filter(|id| id.starts_with("cs_"))
            .ok_or_else(|| Error::Invalid("invalid stripe webhook payload".into()))?
            .to_owned();
        let topup_id = object["metadata"]["aex_topup_id"]
            .as_str()
            .filter(|id| id.starts_with("top_"))
            .ok_or_else(|| Error::Invalid("invalid stripe webhook payload".into()))?
            .to_owned();

        if kind == "checkout.session.expired" {
            return Ok(Some(StripeWebhookAction::Expired {
                topup_id,
                provider_ref,
            }));
        }
        // A completed Checkout Session can still be awaiting a delayed payment method. The
        // asynchronous-success event returns here only after it is paid.
        if !matches!(
            object["payment_status"].as_str(),
            Some("paid" | "no_payment_required")
        ) {
            return Ok(None);
        }
        let amount_cents = object["amount_total"]
            .as_i64()
            .filter(|amount| *amount > 0)
            .ok_or_else(|| Error::Invalid("invalid stripe webhook payload".into()))?;
        Ok(Some(StripeWebhookAction::Paid {
            topup_id,
            provider_ref,
            amount_cents,
        }))
    }
}

fn invalid_webhook() -> Error {
    Error::Invalid("invalid stripe webhook signature".into())
}

#[async_trait::async_trait]
pub trait Payments: Send + Sync {
    fn name(&self) -> &'static str;
    async fn create_checkout(&self, topup_id: &str, amount_cents: i64) -> Result<Checkout>;
    async fn check(&self, provider_ref: &str) -> Result<PaymentStatus>;
}

/// Instantly-paid fake. NOT a payment; local development only.
pub struct FakePayments;

#[async_trait::async_trait]
impl Payments for FakePayments {
    fn name(&self) -> &'static str {
        "fake"
    }

    async fn create_checkout(&self, topup_id: &str, _amount_cents: i64) -> Result<Checkout> {
        Ok(Checkout {
            provider_ref: format!("fake_{topup_id}"),
            url: format!("https://payments.invalid/checkout/{topup_id}"),
        })
    }

    async fn check(&self, _provider_ref: &str) -> Result<PaymentStatus> {
        Ok(PaymentStatus::Paid)
    }
}

/// Stripe Checkout. One Checkout Session per top-up, `usd`, single line item.
pub struct StripePayments {
    http: reqwest::Client,
    secret_key: String,
    success_url: String,
    cancel_url: String,
}

impl StripePayments {
    pub fn new(secret_key: String, success_url: String, cancel_url: String) -> Self {
        StripePayments {
            http: reqwest::Client::new(),
            secret_key,
            success_url,
            cancel_url,
        }
    }
}

#[async_trait::async_trait]
impl Payments for StripePayments {
    fn name(&self) -> &'static str {
        "stripe"
    }

    async fn create_checkout(&self, topup_id: &str, amount_cents: i64) -> Result<Checkout> {
        let amount = amount_cents.to_string();
        let form: Vec<(&str, &str)> = vec![
            ("mode", "payment"),
            ("line_items[0][price_data][currency]", "usd"),
            (
                "line_items[0][price_data][product_data][name]",
                "aex prepaid credit",
            ),
            ("line_items[0][price_data][unit_amount]", &amount),
            ("line_items[0][quantity]", "1"),
            ("success_url", &self.success_url),
            ("cancel_url", &self.cancel_url),
            ("metadata[aex_topup_id]", topup_id),
        ];
        let resp = self
            .http
            .post("https://api.stripe.com/v1/checkout/sessions")
            .bearer_auth(&self.secret_key)
            .form(&form)
            .send()
            .await
            .map_err(|e| Error::Payment(format!("stripe: {e}")))?;
        let status = resp.status();
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| Error::Payment(format!("stripe response: {e}")))?;
        if !status.is_success() {
            let msg = body["error"]["message"].as_str().unwrap_or("unknown error");
            return Err(Error::Payment(format!("stripe {status}: {msg}")));
        }
        match (body["id"].as_str(), body["url"].as_str()) {
            (Some(id), Some(url)) => Ok(Checkout {
                provider_ref: id.to_string(),
                url: url.to_string(),
            }),
            _ => Err(Error::Payment(
                "stripe: checkout session missing id/url".into(),
            )),
        }
    }

    async fn check(&self, provider_ref: &str) -> Result<PaymentStatus> {
        let resp = self
            .http
            .get(format!(
                "https://api.stripe.com/v1/checkout/sessions/{provider_ref}"
            ))
            .bearer_auth(&self.secret_key)
            .send()
            .await
            .map_err(|e| Error::Payment(format!("stripe: {e}")))?;
        let status = resp.status();
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| Error::Payment(format!("stripe response: {e}")))?;
        if !status.is_success() {
            let msg = body["error"]["message"].as_str().unwrap_or("unknown error");
            return Err(Error::Payment(format!("stripe {status}: {msg}")));
        }
        Ok(
            match (body["payment_status"].as_str(), body["status"].as_str()) {
                (Some("paid"), _) | (Some("no_payment_required"), _) => PaymentStatus::Paid,
                (_, Some("expired")) => PaymentStatus::Expired,
                _ => PaymentStatus::Pending,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event() -> Vec<u8> {
        br#"{"type":"checkout.session.completed","data":{"object":{"id":"cs_test_1","amount_total":1000,"payment_status":"paid","metadata":{"aex_topup_id":"top_1"}}}}"#.to_vec()
    }

    fn signature(secret: &str, timestamp: i64, body: &[u8]) -> String {
        let signed = [timestamp.to_string().as_bytes(), b".", body].concat();
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(&signed);
        format!(
            "t={timestamp},v1={}",
            hex::encode(mac.finalize().into_bytes())
        )
    }

    #[test]
    fn verifies_raw_body_and_shapes_paid_checkout() {
        let webhook = StripeWebhook::new("whsec_test".into());
        let body = event();
        let header = signature("whsec_test", 1_000, &body);
        assert_eq!(
            webhook.verify(&header, &body, 1_100).unwrap(),
            Some(StripeWebhookAction::Paid {
                topup_id: "top_1".into(),
                provider_ref: "cs_test_1".into(),
                amount_cents: 1000,
            })
        );
        assert!(webhook.verify(&header, b"{}", 1_100).is_err());
        assert!(webhook.verify(&header, &body, 1_301).is_err());
    }

    #[test]
    fn accepts_one_of_multiple_rotation_signatures_and_ignores_unknown_events() {
        let webhook = StripeWebhook::new("whsec_test".into());
        let body = br#"{"type":"customer.created","data":{"object":{}}}"#;
        let good = signature("whsec_test", 2_000, body);
        let header = good.replacen("v1=", "v1=00,v1=", 1);
        assert_eq!(webhook.verify(&header, body, 2_000).unwrap(), None);
    }
}
