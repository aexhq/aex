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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefundAttempt {
    Pending {
        provider_ref: String,
    },
    Succeeded {
        provider_ref: String,
    },
    Failed {
        provider_ref: Option<String>,
        reason: String,
    },
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
    async fn refund(
        &self,
        topup_id: &str,
        provider_ref: &str,
        refund_id: &str,
        amount_cents: i64,
    ) -> Result<RefundAttempt>;
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

    async fn refund(
        &self,
        _topup_id: &str,
        _provider_ref: &str,
        refund_id: &str,
        _amount_cents: i64,
    ) -> Result<RefundAttempt> {
        Ok(RefundAttempt::Succeeded {
            provider_ref: format!("fake_{refund_id}"),
        })
    }
}

/// Stripe Checkout. One Checkout Session per top-up, `usd`, single line item.
pub struct StripePayments {
    http: reqwest::Client,
    api_base: String,
    secret_key: String,
    success_url: String,
    cancel_url: String,
}

fn checkout_success_url(base: &str) -> String {
    if base.contains("{CHECKOUT_SESSION_ID}") {
        return base.to_owned();
    }
    let (before_fragment, fragment) = base
        .split_once('#')
        .map_or((base, None), |(before, fragment)| (before, Some(fragment)));
    let separator = if before_fragment.contains('?') {
        '&'
    } else {
        '?'
    };
    let mut url = format!("{before_fragment}{separator}session_id={{CHECKOUT_SESSION_ID}}");
    if let Some(fragment) = fragment {
        url.push('#');
        url.push_str(fragment);
    }
    url
}

impl StripePayments {
    pub fn new(secret_key: String, success_url: String, cancel_url: String) -> Self {
        Self::new_with_api_base(
            secret_key,
            success_url,
            cancel_url,
            "https://api.stripe.com".into(),
        )
    }

    fn new_with_api_base(
        secret_key: String,
        success_url: String,
        cancel_url: String,
        api_base: String,
    ) -> Self {
        StripePayments {
            http: reqwest::Client::new(),
            api_base: api_base.trim_end_matches('/').to_owned(),
            secret_key,
            success_url,
            cancel_url,
        }
    }

    async fn payment_intent(&self, topup_id: &str, checkout_ref: &str) -> Result<String> {
        let resp = self
            .http
            .get(format!(
                "{}/v1/checkout/sessions/{checkout_ref}",
                self.api_base
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
        if body["metadata"]["aex_topup_id"].as_str() != Some(topup_id) {
            return Err(Error::Payment(
                "stripe: checkout session top-up metadata mismatch".into(),
            ));
        }
        body["payment_intent"]
            .as_str()
            .filter(|id| id.starts_with("pi_"))
            .map(str::to_owned)
            .ok_or_else(|| Error::Payment("stripe: checkout session missing payment intent".into()))
    }

    /// Reconcile by Aex refund metadata before creating. This remains safe even after Stripe's
    /// idempotency-key retention window has elapsed or the process died after Stripe committed.
    async fn existing_refund(
        &self,
        payment_intent: &str,
        refund_id: &str,
        amount_cents: i64,
    ) -> Result<Option<RefundAttempt>> {
        let mut starting_after: Option<String> = None;
        loop {
            let mut url = format!(
                "{}/v1/refunds?payment_intent={payment_intent}&limit=100",
                self.api_base
            );
            if let Some(cursor) = starting_after.as_deref() {
                url.push_str("&starting_after=");
                url.push_str(cursor);
            }
            let resp = self
                .http
                .get(url)
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
            let data = body["data"]
                .as_array()
                .ok_or_else(|| Error::Payment("stripe: refund list missing data array".into()))?;
            for value in data {
                if value["metadata"]["aex_refund_id"].as_str() == Some(refund_id) {
                    return parse_stripe_refund(value, refund_id, amount_cents).map(Some);
                }
            }
            let has_more = body["has_more"]
                .as_bool()
                .ok_or_else(|| Error::Payment("stripe: refund list missing has_more".into()))?;
            if !has_more {
                return Ok(None);
            }
            starting_after = data
                .last()
                .and_then(|value| value["id"].as_str())
                .map(str::to_owned);
            if starting_after.is_none() {
                return Err(Error::Payment(
                    "stripe: paginated refund list missing cursor".into(),
                ));
            }
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
        let success_url = checkout_success_url(&self.success_url);
        let form: Vec<(&str, &str)> = vec![
            ("mode", "payment"),
            ("line_items[0][price_data][currency]", "usd"),
            (
                "line_items[0][price_data][product_data][name]",
                "Aex prepaid credit",
            ),
            ("line_items[0][price_data][unit_amount]", &amount),
            ("line_items[0][quantity]", "1"),
            ("success_url", &success_url),
            ("cancel_url", &self.cancel_url),
            ("metadata[aex_topup_id]", topup_id),
        ];
        let resp = self
            .http
            .post(format!("{}/v1/checkout/sessions", self.api_base))
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
                "{}/v1/checkout/sessions/{provider_ref}",
                self.api_base
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

    async fn refund(
        &self,
        topup_id: &str,
        provider_ref: &str,
        refund_id: &str,
        amount_cents: i64,
    ) -> Result<RefundAttempt> {
        let payment_intent = self.payment_intent(topup_id, provider_ref).await?;
        if let Some(existing) = self
            .existing_refund(&payment_intent, refund_id, amount_cents)
            .await?
        {
            return Ok(existing);
        }

        let amount = amount_cents.to_string();
        let form = [
            ("payment_intent", payment_intent.as_str()),
            ("amount", amount.as_str()),
            ("reason", "requested_by_customer"),
            ("metadata[aex_refund_id]", refund_id),
        ];
        let resp = self
            .http
            .post(format!("{}/v1/refunds", self.api_base))
            .bearer_auth(&self.secret_key)
            .header("Idempotency-Key", format!("aex-refund:{refund_id}"))
            .form(&form)
            .send()
            .await
            .map_err(|e| Error::Payment(format!("stripe: {e}")))?;
        let status = resp.status();
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| Error::Payment(format!("stripe response: {e}")))?;
        if status.is_client_error() {
            let msg = body["error"]["message"]
                .as_str()
                .unwrap_or("refund rejected")
                .to_owned();
            return Ok(RefundAttempt::Failed {
                provider_ref: None,
                reason: format!("stripe {status}: {msg}"),
            });
        }
        if !status.is_success() {
            let msg = body["error"]["message"].as_str().unwrap_or("unknown error");
            return Err(Error::Payment(format!("stripe {status}: {msg}")));
        }
        parse_stripe_refund(&body, refund_id, amount_cents)
    }
}

fn parse_stripe_refund(
    value: &serde_json::Value,
    refund_id: &str,
    amount_cents: i64,
) -> Result<RefundAttempt> {
    let provider_ref = value["id"]
        .as_str()
        .filter(|id| id.starts_with("re_") || id.starts_with("pyr_"))
        .ok_or_else(|| Error::Payment("stripe: refund missing id".into()))?
        .to_owned();
    if value["metadata"]["aex_refund_id"].as_str() != Some(refund_id)
        || value["amount"].as_i64() != Some(amount_cents)
    {
        return Err(Error::Payment(
            "stripe: refund metadata or amount mismatch".into(),
        ));
    }
    match value["status"].as_str() {
        Some("succeeded") => Ok(RefundAttempt::Succeeded { provider_ref }),
        Some("pending" | "requires_action") => Ok(RefundAttempt::Pending { provider_ref }),
        Some("failed" | "canceled") => Ok(RefundAttempt::Failed {
            provider_ref: Some(provider_ref),
            reason: value["failure_reason"]
                .as_str()
                .unwrap_or("Stripe refund failed")
                .to_owned(),
        }),
        _ => Err(Error::Payment(
            "stripe: refund has an unknown status".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use axum::Json;
    use axum::extract::{Path, State};
    use axum::http::HeaderMap;
    use axum::routing::get;

    use super::*;

    const TEST_TOPUP_ID: &str = "top_01J5X8Y2K3M4N5P6Q7R8S9V2";
    const TEST_REFUND_ID: &str = "rfd_01J5X8Y2K3M4N5P6Q7R8S9W4";

    #[test]
    fn checkout_return_url_carries_the_stripe_session_id() {
        assert_eq!(
            checkout_success_url("https://aex.dev/topup/success"),
            "https://aex.dev/topup/success?session_id={CHECKOUT_SESSION_ID}"
        );
        assert_eq!(
            checkout_success_url("https://aex.dev/topup/success?from=checkout#status"),
            "https://aex.dev/topup/success?from=checkout&session_id={CHECKOUT_SESSION_ID}#status"
        );
        assert_eq!(
            checkout_success_url("https://aex.dev/topup/success?session_id={CHECKOUT_SESSION_ID}"),
            "https://aex.dev/topup/success?session_id={CHECKOUT_SESSION_ID}"
        );
    }

    struct StripeStub {
        existing: bool,
        posts: AtomicUsize,
        idempotency: Mutex<Option<String>>,
        posted_body: Mutex<Option<String>>,
    }

    async fn stripe_stub_checkout(Path(_id): Path<String>) -> Json<serde_json::Value> {
        Json(serde_json::json!({
            "id": "cs_test_1",
            "payment_intent": "pi_test_1",
            "metadata": {"aex_topup_id": TEST_TOPUP_ID}
        }))
    }

    async fn stripe_stub_list(State(stub): State<Arc<StripeStub>>) -> Json<serde_json::Value> {
        let data = if stub.existing {
            vec![serde_json::json!({
                "id": "re_test_existing",
                "amount": 500,
                "status": "succeeded",
                "metadata": {"aex_refund_id": TEST_REFUND_ID}
            })]
        } else {
            Vec::new()
        };
        Json(serde_json::json!({"data": data, "has_more": false}))
    }

    async fn stripe_stub_create(
        State(stub): State<Arc<StripeStub>>,
        headers: HeaderMap,
        body: bytes::Bytes,
    ) -> Json<serde_json::Value> {
        stub.posts.fetch_add(1, Ordering::SeqCst);
        *stub.idempotency.lock().unwrap() = headers
            .get("Idempotency-Key")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        *stub.posted_body.lock().unwrap() = Some(String::from_utf8_lossy(&body).into_owned());
        Json(serde_json::json!({
            "id": "re_test_created",
            "amount": 500,
            "status": "succeeded",
            "metadata": {"aex_refund_id": TEST_REFUND_ID}
        }))
    }

    async fn spawn_stripe_stub(existing: bool) -> (String, Arc<StripeStub>) {
        let stub = Arc::new(StripeStub {
            existing,
            posts: AtomicUsize::new(0),
            idempotency: Mutex::new(None),
            posted_body: Mutex::new(None),
        });
        let app = axum::Router::new()
            .route("/v1/checkout/sessions/{id}", get(stripe_stub_checkout))
            .route(
                "/v1/refunds",
                get(stripe_stub_list).post(stripe_stub_create),
            )
            .with_state(stub.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (base, stub)
    }

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

    #[test]
    fn shapes_refund_provider_states_and_rejects_mismatches() {
        let succeeded = serde_json::json!({
            "id": "re_test_1", "amount": 500, "status": "succeeded",
            "metadata": {"aex_refund_id": "rfd_1"}
        });
        assert_eq!(
            parse_stripe_refund(&succeeded, "rfd_1", 500).unwrap(),
            RefundAttempt::Succeeded {
                provider_ref: "re_test_1".into()
            }
        );
        let payment_refund = serde_json::json!({
            "id": "pyr_test_1", "amount": 500, "status": "succeeded",
            "metadata": {"aex_refund_id": "rfd_1"}
        });
        assert_eq!(
            parse_stripe_refund(&payment_refund, "rfd_1", 500).unwrap(),
            RefundAttempt::Succeeded {
                provider_ref: "pyr_test_1".into()
            }
        );
        let failed = serde_json::json!({
            "id": "re_test_2", "amount": 500, "status": "failed",
            "failure_reason": "expired_or_canceled_card",
            "metadata": {"aex_refund_id": "rfd_2"}
        });
        assert!(matches!(
            parse_stripe_refund(&failed, "rfd_2", 500).unwrap(),
            RefundAttempt::Failed {
                provider_ref: Some(_),
                ..
            }
        ));
        assert!(parse_stripe_refund(&succeeded, "rfd_wrong", 500).is_err());
        assert!(parse_stripe_refund(&succeeded, "rfd_1", 499).is_err());
    }

    #[tokio::test]
    async fn reconciles_durable_refund_metadata_before_posting() {
        let (base, stub) = spawn_stripe_stub(true).await;
        let payments = StripePayments::new_with_api_base(
            "sk_test".into(),
            "https://success.invalid".into(),
            "https://cancel.invalid".into(),
            base,
        );
        let result = payments
            .refund(TEST_TOPUP_ID, "cs_test_1", TEST_REFUND_ID, 500)
            .await
            .unwrap();
        assert!(matches!(result, RefundAttempt::Succeeded { .. }));
        assert_eq!(stub.posts.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn creates_stripe_refund_with_stable_id_and_metadata() {
        let (base, stub) = spawn_stripe_stub(false).await;
        let payments = StripePayments::new_with_api_base(
            "sk_test".into(),
            "https://success.invalid".into(),
            "https://cancel.invalid".into(),
            base,
        );
        let result = payments
            .refund(TEST_TOPUP_ID, "cs_test_1", TEST_REFUND_ID, 500)
            .await
            .unwrap();
        assert!(matches!(result, RefundAttempt::Succeeded { .. }));
        assert_eq!(stub.posts.load(Ordering::SeqCst), 1);
        assert_eq!(
            stub.idempotency.lock().unwrap().as_deref(),
            Some("aex-refund:rfd_01J5X8Y2K3M4N5P6Q7R8S9W4")
        );
        let posted = stub.posted_body.lock().unwrap();
        let posted = posted.as_deref().unwrap();
        assert!(posted.contains("amount=500"), "{posted}");
        assert!(posted.contains(TEST_REFUND_ID), "{posted}");
    }
}
