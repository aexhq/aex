//! The seven-route prepaid billing surface.
//!
//! Provider writes remain durable-effect-first: the exact command is committed
//! before Stripe is contacted and the observed result is committed before it is
//! returned. Public reads expose only integer money, immutable ledger rows,
//! bounded rated usage and verified card display metadata.

use std::sync::Arc;

use aex_finance_domain::money::Microusd;
use aex_internal_contracts::SchemaVersion;
use aex_payment_contracts::{
    CommandKind, EffectMetadata, PaymentCommand, PaymentCommandEnvelope, PaymentResult,
    ProviderIdempotencyKey, TaxMode,
};
use aex_wire::error::{ErrorCode, WireError, WireResult};
use aex_wire::ids::{
    AccountId, BillingTransactionId, OrganizationId, PaymentMethodId, PrefixedId as _, SessionId,
};
use aex_wire::models::{
    BillingBalance, BillingTransaction, BillingTransactionDirection, BillingTransactionKind,
    BillingTransactionPage, BillingTransactionsListQuery, BillingUsageCategory,
    BillingUsageCoverage, BillingUsageGetQuery, BillingUsageItem, BillingUsagePage,
    BillingUsageSettlement, CardBrand, Currency, HostedSession, PaymentMethod, PaymentMethodPage,
    PaymentMethodSessionRequest, ProviderId, TimeRange, TopUpCheckoutRequest,
};
use aex_wire::server::{BillingApi, Created, NoContent, RequestContext};
use aex_wire::types::{Cents, DecimalU128, HttpsUrl, Timestamp};

use super::authority::{
    AuthorityError, BillingAuthority, GatewayError, PaymentGateway, PaymentMethodRecord, UsageQuery,
};

const HOSTED_COMMAND_DEADLINE_MS: i64 = 8_000;

/// The essential prepaid billing service.
#[derive(Debug)]
pub struct BillingService<A, G> {
    authority: Arc<A>,
    gateway: Arc<G>,
    page_limit: u32,
    default_return_url: HttpsUrl,
}

impl<A, G> BillingService<A, G>
where
    A: BillingAuthority,
    G: PaymentGateway,
{
    /// Builds the service over the durable finance authority and Stripe edge.
    #[must_use]
    pub const fn new(
        authority: Arc<A>,
        gateway: Arc<G>,
        page_limit: u32,
        default_return_url: HttpsUrl,
    ) -> Self {
        Self {
            authority,
            gateway,
            page_limit,
            default_return_url,
        }
    }

    /// The authority, for role-scoped readiness checks.
    #[must_use]
    pub fn authority(&self) -> &Arc<A> {
        &self.authority
    }

    fn selected(cx: &RequestContext) -> WireResult<OrganizationId> {
        cx.principal.organization().ok_or_else(|| {
            WireError::new(ErrorCode::Forbidden)
                .with_message("this credential is not bound to the prepaid account")
        })
    }

    fn account_id(organization: OrganizationId) -> AccountId {
        AccountId::from_uuid7(organization.uuid7())
    }

    fn envelope(
        organization: OrganizationId,
        kind: CommandKind,
        effect: aex_payment_contracts::EffectId,
        amount: Option<Microusd>,
        command: PaymentCommand,
        now_millis: i64,
    ) -> WireResult<PaymentCommandEnvelope> {
        Ok(PaymentCommandEnvelope {
            schema_version: SchemaVersion::V1,
            provider_idempotency_key: ProviderIdempotencyKey::derive(kind, organization, effect)
                .as_header_value()
                .as_str()
                .to_owned(),
            deadline: instant(now_millis + HOSTED_COMMAND_DEADLINE_MS)?,
            metadata: EffectMetadata {
                effect,
                organization,
                kind,
                credit: amount.map(cents).transpose()?.unwrap_or(Cents::ZERO),
            },
            command,
        })
    }

    async fn customer(
        &self,
        organization: OrganizationId,
        now_millis: i64,
    ) -> WireResult<aex_payment_contracts::ProviderCustomerRef> {
        if let Some(existing) = self
            .authority
            .provider_customer(organization)
            .await
            .map_err(authority_error)?
        {
            return Ok(existing);
        }
        let email = self
            .authority
            .billing_contact(organization)
            .await
            .map_err(authority_error)?;
        let intent = format!("customer:{}", organization.encode());
        let prepared = self
            .authority
            .prepare_effect(
                organization,
                CommandKind::EnsureCustomer,
                intent.as_bytes(),
                None,
                now_millis + HOSTED_COMMAND_DEADLINE_MS,
            )
            .await
            .map_err(authority_error)?;
        let envelope = Self::envelope(
            organization,
            CommandKind::EnsureCustomer,
            prepared.effect,
            None,
            PaymentCommand::EnsureCustomer {
                effect: prepared.effect,
                organization,
                email,
            },
            now_millis,
        )?;
        self.authority
            .bind_effect_command(prepared.effect, prepared.intent_hash, &envelope)
            .await
            .map_err(authority_error)?;
        let result = self
            .gateway
            .execute(&envelope)
            .await
            .map_err(gateway_error)?;
        self.authority
            .settle_customer(prepared.effect, organization, &result)
            .await
            .map_err(authority_error)?
            .ok_or_else(|| WireError::new(ErrorCode::UpstreamError))
    }

    async fn execute(
        &self,
        organization: OrganizationId,
        kind: CommandKind,
        replay_intent: &str,
        amount: Option<Microusd>,
        command: PaymentCommand,
        now_millis: i64,
    ) -> WireResult<PaymentResult> {
        let prepared = self
            .authority
            .prepare_effect(
                organization,
                kind,
                replay_intent.as_bytes(),
                amount,
                now_millis + HOSTED_COMMAND_DEADLINE_MS,
            )
            .await
            .map_err(authority_error)?;
        let command = match command {
            PaymentCommand::CreateTopUpCheckout {
                organization,
                amount,
                success_url,
                cancel_url,
                customer,
                tax,
                ..
            } => PaymentCommand::CreateTopUpCheckout {
                effect: prepared.effect,
                organization,
                amount,
                success_url,
                cancel_url,
                customer,
                tax,
            },
            PaymentCommand::CreatePaymentMethodSession {
                organization,
                success_url,
                cancel_url,
                customer,
                consented_at,
                ..
            } => PaymentCommand::CreatePaymentMethodSession {
                effect: prepared.effect,
                organization,
                success_url,
                cancel_url,
                customer,
                consented_at,
            },
            PaymentCommand::DetachPaymentMethod {
                organization,
                customer,
                method,
                ..
            } => PaymentCommand::DetachPaymentMethod {
                effect: prepared.effect,
                organization,
                customer,
                method,
            },
            _ => return Err(WireError::new(ErrorCode::InternalError)),
        };
        let envelope = Self::envelope(
            organization,
            kind,
            prepared.effect,
            amount,
            command,
            now_millis,
        )?;
        self.authority
            .bind_effect_command(prepared.effect, prepared.intent_hash, &envelope)
            .await
            .map_err(authority_error)?;
        let result = self
            .gateway
            .execute(&envelope)
            .await
            .map_err(gateway_error)?;
        self.authority
            .finalize_effect(prepared.effect, &result)
            .await
            .map_err(authority_error)?;
        Ok(result)
    }

    fn replay_intent(cx: &RequestContext, body: impl serde::Serialize) -> WireResult<String> {
        let replay = cx.idempotency_key.as_ref().ok_or_else(|| {
            WireError::new(ErrorCode::InvalidRequest).with_message("Idempotency-Key is required")
        })?;
        let canonical = serde_json::to_vec(&body).map_err(|error| {
            WireError::new(ErrorCode::InternalError).with_message(error.to_string())
        })?;
        Ok(format!(
            "{}:{}:{}",
            cx.route.as_str(),
            replay.as_str(),
            blake3::hash(&canonical)
        ))
    }

    fn hosted(result: PaymentResult) -> WireResult<Created<HostedSession>> {
        match result {
            PaymentResult::Succeeded {
                hosted: Some(hosted),
                ..
            } => Ok(Created(HostedSession {
                url: hosted.url,
                expires_at: hosted.expires_at,
            })),
            PaymentResult::Succeeded { .. } => Err(WireError::new(ErrorCode::UpstreamError)
                .with_message("the provider created no hosted page")),
            PaymentResult::Failed { .. } | PaymentResult::Unknown { .. } => {
                Err(WireError::new(ErrorCode::UpstreamError))
            }
        }
    }
}

impl<A, G> BillingApi for BillingService<A, G>
where
    A: BillingAuthority,
    G: PaymentGateway,
{
    async fn billing_balance_get(&self, cx: &RequestContext) -> WireResult<BillingBalance> {
        let organization = Self::selected(cx)?;
        let record = self
            .authority
            .balance(organization)
            .await
            .map_err(authority_error)?;
        Ok(BillingBalance {
            account_id: Self::account_id(organization),
            available_cents: cents(record.available)?,
            currency: Currency::USD,
            pending_cents: cents(record.pending)?,
            reserved_cents: cents(record.reserved)?,
            updated_at: instant(record.updated_at_millis)?,
        })
    }

    async fn billing_payment_methods_list(
        &self,
        cx: &RequestContext,
    ) -> WireResult<PaymentMethodPage> {
        let organization = Self::selected(cx)?;
        let records = self
            .authority
            .payment_methods(organization)
            .await
            .map_err(authority_error)?;
        Ok(PaymentMethodPage {
            items: records
                .into_iter()
                .map(public_payment_method)
                .collect::<WireResult<Vec<_>>>()?,
        })
    }

    async fn billing_payment_method_session_create(
        &self,
        cx: &RequestContext,
        body: PaymentMethodSessionRequest,
    ) -> WireResult<Created<HostedSession>> {
        if !body.consent {
            return Err(WireError::new(ErrorCode::InvalidRequest)
                .with_message("explicit consent is required to save a card"));
        }
        let organization = Self::selected(cx)?;
        let now = now()?;
        let customer = self.customer(organization, now.unix_millis()).await?;
        let intent = Self::replay_intent(cx, &body)?;
        let result = self
            .execute(
                organization,
                CommandKind::CreatePaymentMethodSession,
                &intent,
                None,
                PaymentCommand::CreatePaymentMethodSession {
                    effect: placeholder_effect()?,
                    organization,
                    success_url: body
                        .success_url
                        .unwrap_or_else(|| self.default_return_url.clone()),
                    cancel_url: body
                        .cancel_url
                        .unwrap_or_else(|| self.default_return_url.clone()),
                    customer,
                    consented_at: now,
                },
                now.unix_millis(),
            )
            .await?;
        Self::hosted(result)
    }

    async fn billing_payment_method_delete(
        &self,
        cx: &RequestContext,
        payment_method_id: PaymentMethodId,
    ) -> WireResult<NoContent> {
        let organization = Self::selected(cx)?;
        let record = self
            .authority
            .payment_method(
                organization,
                uuid::Uuid::from_bytes(*payment_method_id.uuid7().as_bytes()),
            )
            .await
            .map_err(authority_error)?;
        let customer = self
            .authority
            .provider_customer(organization)
            .await
            .map_err(authority_error)?
            .ok_or_else(|| WireError::new(ErrorCode::NotFound))?;
        let now = now()?;
        let intent = Self::replay_intent(cx, payment_method_id)?;
        let result = self
            .execute(
                organization,
                CommandKind::DetachPaymentMethod,
                &intent,
                None,
                PaymentCommand::DetachPaymentMethod {
                    effect: placeholder_effect()?,
                    organization,
                    customer,
                    method: record.provider_method,
                },
                now.unix_millis(),
            )
            .await?;
        match result {
            PaymentResult::Succeeded { .. } => Ok(NoContent),
            PaymentResult::Failed { .. } | PaymentResult::Unknown { .. } => {
                Err(WireError::new(ErrorCode::UpstreamError))
            }
        }
    }

    async fn billing_top_up_checkout_create(
        &self,
        cx: &RequestContext,
        body: TopUpCheckoutRequest,
    ) -> WireResult<Created<HostedSession>> {
        let organization = Self::selected(cx)?;
        let cents = aex_finance_domain::Cents::new(
            i64::try_from(body.amount_cents.get()).map_err(|_| {
                WireError::new(ErrorCode::InvalidRequest).with_message("top-up amount is too large")
            })?,
        )
        .map_err(|error| {
            WireError::new(ErrorCode::InvalidRequest).with_message(error.to_string())
        })?;
        let amount = cents.to_microusd();
        let now = now()?;
        let customer = self.customer(organization, now.unix_millis()).await?;
        let intent = Self::replay_intent(cx, &body)?;
        let result = self
            .execute(
                organization,
                CommandKind::CreateTopUpCheckout,
                &intent,
                Some(amount),
                PaymentCommand::CreateTopUpCheckout {
                    effect: placeholder_effect()?,
                    organization,
                    amount: body.amount_cents,
                    success_url: body
                        .success_url
                        .unwrap_or_else(|| self.default_return_url.clone()),
                    cancel_url: body
                        .cancel_url
                        .unwrap_or_else(|| self.default_return_url.clone()),
                    customer,
                    tax: TaxMode::ProviderAutomatic,
                },
                now.unix_millis(),
            )
            .await?;
        Self::hosted(result)
    }

    async fn billing_transactions_list(
        &self,
        cx: &RequestContext,
        query: BillingTransactionsListQuery,
    ) -> WireResult<BillingTransactionPage> {
        let organization = Self::selected(cx)?;
        let after = query
            .cursor
            .as_ref()
            .map(|cursor| decode_cursor_uuid(cursor.as_str()))
            .transpose()?;
        let record = self
            .authority
            .transactions(
                organization,
                after,
                query.limit.unwrap_or(self.page_limit).min(100),
            )
            .await
            .map_err(authority_error)?;
        Ok(BillingTransactionPage {
            items: record
                .items
                .into_iter()
                .map(|item| {
                    let signed = item.available_delta;
                    Ok(BillingTransaction {
                        amount_cents: cents(
                            Microusd::new(signed.unsigned_abs().cast_signed()).map_err(
                                |error| {
                                    WireError::new(ErrorCode::InternalError)
                                        .with_message(error.to_string())
                                },
                            )?,
                        )?,
                        balance_after_cents: cents(
                            Microusd::new(item.balance_after.max(0)).map_err(|error| {
                                WireError::new(ErrorCode::InternalError)
                                    .with_message(error.to_string())
                            })?,
                        )?,
                        currency: Currency::USD,
                        direction: if signed >= 0 {
                            BillingTransactionDirection::Credit
                        } else {
                            BillingTransactionDirection::Debit
                        },
                        id: billing_transaction_id(item.id)?,
                        kind: transaction_kind(&item.kind)?,
                        occurred_at: instant(item.occurred_at_millis)?,
                        reverses_transaction_id: item
                            .reverses
                            .map(billing_transaction_id)
                            .transpose()?,
                        session_id: item
                            .session_id
                            .as_deref()
                            .map(SessionId::parse)
                            .transpose()
                            .map_err(|error| {
                                WireError::new(ErrorCode::InternalError)
                                    .with_message(error.to_string())
                            })?,
                    })
                })
                .collect::<WireResult<Vec<_>>>()?,
            next_cursor: record.next.map(encode_cursor_uuid).transpose()?,
        })
    }

    async fn billing_usage_get(
        &self,
        cx: &RequestContext,
        query: BillingUsageGetQuery,
    ) -> WireResult<BillingUsagePage> {
        let organization = Self::selected(cx)?;
        if query
            .from
            .zip(query.to)
            .is_some_and(|(from, to)| from >= to)
        {
            return Err(WireError::new(ErrorCode::InvalidRequest)
                .with_message("from must be earlier than to"));
        }
        let after_sequence = query
            .cursor
            .as_ref()
            .map(|cursor| decode_cursor_i64(cursor.as_str()))
            .transpose()?;
        let session = query.session_id.map(|id| id.to_string());
        let record = self
            .authority
            .usage(
                organization,
                UsageQuery {
                    after_sequence,
                    category: query.category.map(BillingUsageCategory::as_str),
                    session_id: session.as_deref(),
                    from_millis: query.from.map(Timestamp::unix_millis),
                    to_millis: query.to.map(Timestamp::unix_millis),
                    limit: query.limit.unwrap_or(self.page_limit).min(100),
                },
            )
            .await
            .map_err(authority_error)?;
        Ok(BillingUsagePage {
            coverage: BillingUsageCoverage {
                complete_through: instant(record.complete_through_millis)?,
                has_gap: record.has_gap,
                includes_through: instant(record.includes_through_millis)?,
            },
            items: record
                .items
                .into_iter()
                .map(|item| {
                    Ok(BillingUsageItem {
                        amount_cents: cents(item.rated)?,
                        category: usage_category(&item.category)?,
                        charged_quantity: DecimalU128::new(item.charged_quantity),
                        currency: Currency::USD,
                        model: item.model,
                        provider: item.provider.as_deref().map(provider_id).transpose()?,
                        quantity: DecimalU128::new(item.quantity),
                        service_time: TimeRange {
                            gte: instant(item.from_millis)?,
                            lt: instant(item.to_millis)?,
                        },
                        session_id: item
                            .session_id
                            .as_deref()
                            .map(SessionId::parse)
                            .transpose()
                            .map_err(|error| {
                                WireError::new(ErrorCode::InternalError)
                                    .with_message(error.to_string())
                            })?,
                        settlement: if item.settled {
                            BillingUsageSettlement::Settled
                        } else {
                            BillingUsageSettlement::Pending
                        },
                        unit: item.meter,
                    })
                })
                .collect::<WireResult<Vec<_>>>()?,
            next_cursor: record.next_sequence.map(encode_cursor_i64).transpose()?,
        })
    }
}

fn public_payment_method(record: PaymentMethodRecord) -> WireResult<PaymentMethod> {
    let id =
        PaymentMethodId::from_uuid7(aex_wire::Uuid7::from_bytes(*record.id.as_bytes()).map_err(
            |error| WireError::new(ErrorCode::InternalError).with_message(error.to_string()),
        )?);
    Ok(PaymentMethod {
        brand: card_brand(&record.brand),
        created_at: instant(record.created_at_millis)?,
        expiry_month: record.expiry_month,
        expiry_year: record.expiry_year,
        id,
        is_default: record.is_default,
        last4: record.last4,
    })
}

fn card_brand(value: &str) -> CardBrand {
    match value {
        "visa" => CardBrand::Visa,
        "mastercard" => CardBrand::Mastercard,
        "amex" => CardBrand::Amex,
        "discover" => CardBrand::Discover,
        "diners" => CardBrand::Diners,
        "jcb" => CardBrand::Jcb,
        "unionpay" => CardBrand::Unionpay,
        _ => CardBrand::Unknown,
    }
}

fn transaction_kind(value: &str) -> WireResult<BillingTransactionKind> {
    match value {
        "top_up_settled" => Ok(BillingTransactionKind::TopUp),
        "usage_settlement" => Ok(BillingTransactionKind::Usage),
        "refund" => Ok(BillingTransactionKind::Refund),
        "reversal" => Ok(BillingTransactionKind::Reversal),
        "usage_reserve" => Ok(BillingTransactionKind::Reservation),
        "reservation_release" => Ok(BillingTransactionKind::ReservationRelease),
        other => Err(
            WireError::new(ErrorCode::InternalError).with_message(format!(
                "unknown customer-visible transaction kind: {other}"
            )),
        ),
    }
}

fn usage_category(value: &str) -> WireResult<BillingUsageCategory> {
    match value {
        "model" | "model_tokens" => Ok(BillingUsageCategory::Model),
        "runtime" | "runtime_compute" => Ok(BillingUsageCategory::Runtime),
        "storage" => Ok(BillingUsageCategory::Storage),
        "transfer" | "egress" => Ok(BillingUsageCategory::Transfer),
        other => Err(WireError::new(ErrorCode::InternalError)
            .with_message(format!("unknown usage category: {other}"))),
    }
}

fn provider_id(value: &str) -> WireResult<ProviderId> {
    match value {
        "openai" => Ok(ProviderId::Openai),
        "anthropic" => Ok(ProviderId::Anthropic),
        "deepseek" => Ok(ProviderId::Deepseek),
        "xai" => Ok(ProviderId::Xai),
        "meta" => Ok(ProviderId::Meta),
        "moonshotai" => Ok(ProviderId::Moonshotai),
        "alibaba" => Ok(ProviderId::Alibaba),
        other => Err(WireError::new(ErrorCode::InternalError)
            .with_message(format!("unknown official provider: {other}"))),
    }
}

fn cents(amount: Microusd) -> WireResult<Cents> {
    let exact = amount.to_cents_exact().map_err(|error| {
        WireError::new(ErrorCode::InternalError)
            .with_message(format!("stored money is not an exact cent amount: {error}"))
    })?;
    Ok(Cents::new(u64::try_from(exact.get()).map_err(|_| {
        WireError::new(ErrorCode::InternalError).with_message("stored money is negative")
    })?))
}

fn instant(millis: i64) -> WireResult<Timestamp> {
    Timestamp::from_unix_millis(millis)
        .map_err(|error| WireError::new(ErrorCode::InternalError).with_message(error.to_string()))
}

fn now() -> WireResult<Timestamp> {
    Timestamp::from_datetime_trunc_ms(time::OffsetDateTime::now_utc())
        .map_err(|error| WireError::new(ErrorCode::InternalError).with_message(error.to_string()))
}

fn billing_transaction_id(value: uuid::Uuid) -> WireResult<BillingTransactionId> {
    Ok(BillingTransactionId::from_uuid7(
        aex_wire::Uuid7::from_bytes(*value.as_bytes()).map_err(|error| {
            WireError::new(ErrorCode::InternalError).with_message(error.to_string())
        })?,
    ))
}

fn placeholder_effect() -> WireResult<aex_payment_contracts::EffectId> {
    let bytes = uuid::Uuid::now_v7();
    Ok(aex_payment_contracts::EffectId(
        aex_wire::Uuid7::from_bytes(*bytes.as_bytes()).map_err(|error| {
            WireError::new(ErrorCode::InternalError).with_message(error.to_string())
        })?,
    ))
}

fn encode_cursor_uuid(value: uuid::Uuid) -> WireResult<aex_wire::Cursor> {
    aex_wire::Cursor::parse(&format!("cur_{}", value.simple()))
        .map_err(|error| WireError::new(ErrorCode::InternalError).with_message(error.to_string()))
}

fn decode_cursor_uuid(value: &str) -> WireResult<uuid::Uuid> {
    value
        .strip_prefix("cur_")
        .and_then(|raw| uuid::Uuid::parse_str(raw).ok())
        .ok_or_else(|| WireError::new(ErrorCode::InvalidCursor))
}

fn encode_cursor_i64(value: i64) -> WireResult<aex_wire::Cursor> {
    aex_wire::Cursor::parse(&format!("cur_{value}"))
        .map_err(|error| WireError::new(ErrorCode::InternalError).with_message(error.to_string()))
}

fn decode_cursor_i64(value: &str) -> WireResult<i64> {
    value
        .strip_prefix("cur_")
        .and_then(|raw| raw.parse().ok())
        .ok_or_else(|| WireError::new(ErrorCode::InvalidCursor))
}

fn authority_error(error: AuthorityError) -> WireError {
    match error {
        AuthorityError::UnknownOrganization | AuthorityError::NotFound(_) => {
            WireError::new(ErrorCode::NotFound)
        }
        AuthorityError::RevisionConflict => WireError::new(ErrorCode::PreconditionFailed),
        AuthorityError::EffectAlreadyOpen => WireError::new(ErrorCode::IdempotencyConflict),
        AuthorityError::Refused(reason) => {
            WireError::new(ErrorCode::InvalidRequest).with_message(reason)
        }
        AuthorityError::Unavailable(_) | AuthorityError::OutcomeUnknown(_) => {
            WireError::new(ErrorCode::UpstreamError)
        }
        AuthorityError::Decode(reason) => {
            WireError::new(ErrorCode::InternalError).with_message(reason)
        }
    }
}

fn gateway_error(error: GatewayError) -> WireError {
    match error {
        GatewayError::Unavailable(_) | GatewayError::OutcomeUnknown(_) => {
            WireError::new(ErrorCode::UpstreamError)
        }
        GatewayError::OffContract(reason) => {
            WireError::new(ErrorCode::InternalError).with_message(reason)
        }
    }
}
