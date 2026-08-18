//! The payments seam: how a top-up gets paid.
//!
//! Same adapter posture as the brain's substrate seams. `Stripe` is the production
//! implementation (Checkout Session per top-up; the customer pays in the browser). `Fake` is
//! the zero-config local default — every checkout is instantly "paid", no money moves, and the
//! server banners that loudly. Crediting is not done here: the caller marks the topup paid in
//! the store, idempotently, when `check` reports Paid — so webhook delivery and polling can
//! both drive it, in any order, any number of times.

use crate::{Error, Result};

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
