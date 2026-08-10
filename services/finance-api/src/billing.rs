//! The generated `central:billing` server trait, implemented over the ports.
//!
//! Money leaves this module as [`aex_wire::types::Cents`], and the only path
//! from micro-USD to cents is [`Microusd::to_cents_exact`], which refuses a
//! sub-cent remainder. There is no rounding at the public boundary, because a
//! balance that rounds is a balance a customer can dispute.

use std::sync::Arc;

use aex_control_domain::{AccountProfile, AccountProjectionError, account_operational_state};
use aex_finance_domain::money::Microusd;
use aex_internal_contracts::SchemaVersion;
use aex_payment_contracts::{
    CommandKind, EffectMetadata, PaymentCommand, PaymentCommandEnvelope, PaymentResult,
    ProviderIdempotencyKey, TaxMode,
};
use aex_wire::error::{ErrorCode, WireError, WireResult};
use aex_wire::ids::{MeasurementId, OrganizationId, PrefixedId as _, StatementId};
use aex_wire::models::Currency;
use aex_wire::models::{
    AccountOperationalState, AutoTopupPolicy, AutoTopupPolicyRequest, BillingBalance,
    BillingBalanceGetQuery, BillingStatementsListQuery, DownloadGrant, EmptyRequest, HostedSession,
    PortalSessionRequest, Statement, StatementLine, StatementSummary, StatementSummaryPage,
    TimeRange, TopUpCheckoutRequest, UsageCategory,
};
use aex_wire::server::{BillingApi, Created, RequestContext, WithETag};
use aex_wire::types::{Cents, DecimalU128, ETag, HttpsUrl, Timestamp};

use crate::authority::{
    AuthorityError, BillingAuthority, GatewayError, PaymentGateway, PolicyChange, PolicyRecord,
    StatementHeader,
};
use crate::download::StatementDownloads;

/// How long a hosted provider page may take to be created.
const HOSTED_COMMAND_DEADLINE_MS: i64 = 8_000;

/// Everything the eight billing routes need.
#[derive(Debug)]
pub struct BillingService<A, G, D> {
    authority: Arc<A>,
    gateway: Arc<G>,
    downloads: Arc<D>,
    page_limit: u32,
    download_grant_ttl_ms: i64,
    /// Where a hosted page returns the caller when it has no preference.
    default_return_url: HttpsUrl,
}

impl<A, G, D> BillingService<A, G, D>
where
    A: BillingAuthority,
    G: PaymentGateway,
    D: StatementDownloads,
{
    /// Builds the service over its three ports.
    #[must_use]
    pub const fn new(
        authority: Arc<A>,
        gateway: Arc<G>,
        downloads: Arc<D>,
        page_limit: u32,
        download_grant_ttl_ms: i64,
        default_return_url: HttpsUrl,
    ) -> Self {
        Self {
            authority,
            gateway,
            downloads,
            page_limit,
            download_grant_ttl_ms,
            default_return_url,
        }
    }

    /// The authority, for the readiness probe.
    #[must_use]
    pub fn authority(&self) -> &Arc<A> {
        &self.authority
    }

    /// Refuses a request whose principal does not act in `organization`.
    ///
    /// A workspace key is pinned to exactly one organization; an account token
    /// carries the organization the caller selected. Either way, a mismatch is
    /// `forbidden` rather than `not_found`, because the caller does hold a
    /// credential — it just is not for this account.
    fn authorize(cx: &RequestContext, organization: OrganizationId) -> WireResult<()> {
        match cx.principal.organization() {
            Some(actual) if actual == organization => Ok(()),
            Some(_) => Err(WireError::new(ErrorCode::Forbidden)),
            None => Err(WireError::new(ErrorCode::Forbidden)
                .with_message("this credential is not bound to an organization; select one first")),
        }
    }

    /// Resolves the organization a balance read is about.
    fn selected(cx: &RequestContext, query: &BillingBalanceGetQuery) -> WireResult<OrganizationId> {
        match (query.organization_id, cx.principal.organization()) {
            (Some(asked), Some(actual)) if asked == actual => Ok(asked),
            (Some(_), Some(_)) => Err(WireError::new(ErrorCode::Forbidden)),
            (Some(_) | None, None) => Err(WireError::new(ErrorCode::Forbidden)
                .with_message("this credential is not bound to an organization")),
            (None, Some(actual)) => Ok(actual),
        }
    }

    /// Wraps one admitted command in the envelope the edge executes.
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
            deadline: Timestamp::from_unix_millis(now_millis + HOSTED_COMMAND_DEADLINE_MS)
                .map_err(|error| {
                    WireError::new(ErrorCode::InternalError)
                        .with_message(format!("the command deadline is unrepresentable: {error}"))
                })?,
            metadata: EffectMetadata {
                effect,
                organization,
                kind,
                credit: amount
                    .and_then(|value| value.to_cents_exact().ok())
                    .map_or(Cents::ZERO, |cents| {
                        Cents::new(u64::try_from(cents.get()).unwrap_or(0))
                    }),
            },
            command,
        })
    }

    /// Resolves the organization's provider customer, creating it on first use.
    ///
    /// This is the forward path `CommandKind::EnsureCustomer` never had. Both
    /// hosted commands act on a provider customer, and nothing created one, so
    /// every top-up and every portal session refused with `400 invalid_request`
    /// and no money could enter the system at all.
    ///
    /// Creation is at most once per organization by three independent fences: the
    /// intent is the organization, so a replay resolves the original effect rather
    /// than opening a second; the derived provider idempotency key makes a second
    /// dispatch of that effect return the same customer; and the column is claimed
    /// under `provider_customer_id IS NULL`, so a concurrent winner is adopted
    /// rather than overwritten.
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
        let intent = format!("customer:{}", organization.encode().as_str());
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

    /// Executes one hosted-page command through prepare, execute and finalize.
    ///
    /// The effect is durable before the provider is contacted and the answer is
    /// durable before the caller sees it. An indeterminate answer is recorded as
    /// `outcome_unknown` and reported as `upstream_error`; it is never retried
    /// here and never becomes a determinate refusal.
    async fn hosted(
        &self,
        organization: OrganizationId,
        kind: CommandKind,
        intent: &[u8],
        amount: Option<Microusd>,
        build: impl FnOnce(
            aex_payment_contracts::EffectId,
            aex_payment_contracts::ProviderCustomerRef,
        ) -> PaymentCommand
        + Send,
        now_millis: i64,
    ) -> WireResult<HostedSession> {
        let customer = self.customer(organization, now_millis).await?;
        let prepared = self
            .authority
            .prepare_effect(
                organization,
                kind,
                intent,
                amount,
                now_millis + HOSTED_COMMAND_DEADLINE_MS,
            )
            .await
            .map_err(authority_error)?;
        let envelope = Self::envelope(
            organization,
            kind,
            prepared.effect,
            amount,
            build(prepared.effect, customer),
            now_millis,
        )?;

        if kind != CommandKind::CreatePortalSession {
            self.authority
                .bind_effect_command(prepared.effect, prepared.intent_hash, &envelope)
                .await
                .map_err(authority_error)?;
        }

        let result = self
            .gateway
            .execute(&envelope)
            .await
            .map_err(gateway_error)?;
        self.authority
            .finalize_effect(prepared.effect, &result)
            .await
            .map_err(authority_error)?;

        match result {
            PaymentResult::Succeeded {
                hosted: Some(hosted),
                ..
            } => Ok(HostedSession {
                url: hosted.url,
                expires_at: hosted.expires_at,
            }),
            PaymentResult::Succeeded { .. } => Err(WireError::new(ErrorCode::UpstreamError)
                .with_message("the provider created no hosted page")),
            PaymentResult::Failed { .. } | PaymentResult::Unknown { .. } => {
                Err(WireError::new(ErrorCode::UpstreamError))
            }
        }
    }
}

/// Maps an authority failure onto a declared public code.
fn authority_error(error: AuthorityError) -> WireError {
    match error {
        AuthorityError::UnknownOrganization | AuthorityError::NotFound(_) => {
            WireError::new(ErrorCode::NotFound)
        }
        AuthorityError::RevisionConflict => WireError::new(ErrorCode::PreconditionFailed),
        AuthorityError::EffectAlreadyOpen => WireError::new(ErrorCode::IdempotencyConflict)
            .with_message("an unresolved provider effect already exists for this organization"),
        AuthorityError::Refused(reason) => {
            WireError::new(ErrorCode::InvalidRequest).with_message(reason)
        }
        AuthorityError::Unavailable(_) | AuthorityError::OutcomeUnknown(_) => {
            // A lost commit is not reported as a failure the caller may treat as
            // "nothing happened": the retry is safe under the same replay
            // identity, which resolves the original transition.
            //
            // TODO(cross-stream): contracts — no billing route declares an
            // unavailability code, so `dispatch::declared` currently renders
            // this as `internal_error`. The code is emitted honestly here so the
            // gap closes the moment the route table admits it.
            WireError::new(ErrorCode::AccountStateUnavailable)
        }
        AuthorityError::Decode(reason) => WireError::new(ErrorCode::InternalError).with_message(
            format!("the finance authority answered off-contract: {reason}"),
        ),
    }
}

/// Maps a gateway failure onto a declared public code.
fn gateway_error(error: GatewayError) -> WireError {
    match error {
        GatewayError::Unavailable(_) | GatewayError::OutcomeUnknown(_) => {
            WireError::new(ErrorCode::UpstreamError)
        }
        GatewayError::OffContract(reason) => WireError::new(ErrorCode::InternalError).with_message(
            format!("the payment command edge answered off-contract: {reason}"),
        ),
    }
}

/// Converts an exact micro-USD amount to whole cents, refusing a remainder.
fn cents(amount: Microusd) -> WireResult<Cents> {
    let exact = amount.to_cents_exact().map_err(|error| {
        WireError::new(ErrorCode::InternalError)
            .with_message(format!("a stored amount is not a whole cent: {error}"))
    })?;
    let raw = u64::try_from(exact.get()).map_err(|_| {
        WireError::new(ErrorCode::InternalError)
            .with_message("a stored amount is negative on the public surface")
    })?;
    Ok(Cents::new(raw))
}

/// Renders an epoch-millisecond instant, or refuses it.
fn instant(millis: i64) -> WireResult<Timestamp> {
    Timestamp::from_unix_millis(millis).map_err(|error| {
        WireError::new(ErrorCode::InternalError)
            .with_message(format!("a stored instant is unrepresentable: {error}"))
    })
}

/// Projects an account through the one shared mapping and maps its refusals.
///
/// Finance publishes the projection's answer and derives nothing of its own. It
/// used to read `state` beside a live balance and call a funded-but-held account
/// paused for a reason it invented, which meant a customer under a dispute hold
/// was told to make a payment that would not restore service.
fn operational_state(profile: &AccountProfile) -> WireResult<AccountOperationalState> {
    account_operational_state(profile).map_err(|error| match error {
        AccountProjectionError::Unavailable => WireError::new(ErrorCode::AccountStateUnavailable),
        AccountProjectionError::MissingPauseCause
        | AccountProjectionError::UnknownPauseCause(_)
        | AccountProjectionError::UnrepresentableInstant => {
            WireError::new(ErrorCode::InternalError)
                .with_message("the account state projection is off-contract")
        }
    })
}

/// The entity tag of a policy revision.
fn policy_etag(revision: u64) -> WireResult<ETag> {
    ETag::parse(&format!("\"policy-{revision}\"")).map_err(|error| {
        WireError::new(ErrorCode::InternalError)
            .with_message(format!("the policy entity tag is unrepresentable: {error}"))
    })
}

/// The public policy view of one durable record.
fn policy_view(organization: OrganizationId, record: &PolicyRecord) -> WireResult<AutoTopupPolicy> {
    Ok(AutoTopupPolicy {
        amount_cents: cents(record.amount)?,
        enabled: record.enabled,
        organization_id: organization,
        revision: record.revision,
        threshold_cents: cents(record.threshold)?,
        updated_at: instant(record.updated_at_millis)?,
    })
}

/// The `YYYY-MM` period as a half-open instant range.
fn period_range(period: &str) -> WireResult<TimeRange> {
    let unrepresentable = || {
        WireError::new(ErrorCode::InternalError)
            .with_message(format!("statement period `{period}` is unrepresentable"))
    };
    let (year, month) = period.split_once('-').ok_or_else(unrepresentable)?;
    let year: i32 = year.parse().map_err(|_| unrepresentable())?;
    let month: u8 = month.parse().map_err(|_| unrepresentable())?;
    let month = time::Month::try_from(month).map_err(|_| unrepresentable())?;
    let start = time::Date::from_calendar_date(year, month, 1).map_err(|_| unrepresentable())?;
    let end = if month == time::Month::December {
        time::Date::from_calendar_date(year + 1, time::Month::January, 1)
    } else {
        time::Date::from_calendar_date(year, month.next(), 1)
    }
    .map_err(|_| unrepresentable())?;
    Ok(TimeRange {
        gte: Timestamp::from_datetime_trunc_ms(start.midnight().assume_utc())
            .map_err(|_| unrepresentable())?,
        lt: Timestamp::from_datetime_trunc_ms(end.midnight().assume_utc())
            .map_err(|_| unrepresentable())?,
    })
}

/// The public summary of one issued statement.
fn summary(organization: OrganizationId, header: &StatementHeader) -> WireResult<StatementSummary> {
    Ok(StatementSummary {
        artifact_hash: aex_wire::ids::ContentHash::from_bytes(header.content_sha256.ok_or_else(
            || {
                WireError::new(ErrorCode::InternalError)
                    .with_message("an issued statement has no artifact digest")
            },
        )?),
        currency: Currency::USD,
        id: StatementId::from_uuid7(
            aex_wire::Uuid7::from_bytes(*header.statement_id.as_bytes()).map_err(|error| {
                WireError::new(ErrorCode::InternalError)
                    .with_message(format!("a statement id is not a UUIDv7: {error}"))
            })?,
        ),
        issued_at: instant(header.issued_at_millis)?,
        organization_id: organization,
        period: period_range(&header.period)?,
        total_cents: cents(header.closing)?,
    })
}

/// The usage category spelling the inbox stores.
fn category(raw: &str) -> WireResult<UsageCategory> {
    match raw {
        "storage" => Ok(UsageCategory::Storage),
        "compute" => Ok(UsageCategory::Compute),
        "memory" => Ok(UsageCategory::Memory),
        "transfer" | "data_transfer" => Ok(UsageCategory::DataTransfer),
        other => Err(WireError::new(ErrorCode::InternalError)
            .with_message(format!("unknown usage category `{other}`"))),
    }
}

impl<A, G, D> BillingApi for BillingService<A, G, D>
where
    A: BillingAuthority,
    G: PaymentGateway,
    D: StatementDownloads,
{
    async fn billing_auto_topup_policy_get(
        &self,
        cx: &RequestContext,
        organization_id: OrganizationId,
    ) -> WireResult<WithETag<AutoTopupPolicy>> {
        Self::authorize(cx, organization_id)?;
        let record = self
            .authority
            .policy(organization_id)
            .await
            .map_err(authority_error)?;
        Ok(WithETag {
            etag: policy_etag(record.revision)?,
            value: policy_view(organization_id, &record)?,
        })
    }

    async fn billing_auto_topup_policy_put(
        &self,
        cx: &RequestContext,
        organization_id: OrganizationId,
        body: AutoTopupPolicyRequest,
    ) -> WireResult<WithETag<AutoTopupPolicy>> {
        Self::authorize(cx, organization_id)?;
        let expected = cx
            .if_match
            .as_ref()
            .ok_or_else(|| WireError::new(ErrorCode::PreconditionFailed))?;
        let current = self
            .authority
            .policy(organization_id)
            .await
            .map_err(authority_error)?;
        if policy_etag(current.revision)?.as_str() != expected.as_str() {
            return Err(WireError::new(ErrorCode::PreconditionFailed));
        }
        if body.enabled && !current.has_payment_method {
            return Err(WireError::new(ErrorCode::PaymentMethodRequired));
        }
        let change = PolicyChange {
            enabled: body.enabled,
            threshold: Microusd::new(
                i64::try_from(body.threshold_cents.get())
                    .ok()
                    .and_then(|value| value.checked_mul(aex_finance_domain::MICROUSD_PER_CENT))
                    .ok_or_else(|| WireError::new(ErrorCode::InvalidAutoTopupPolicy))?,
            )
            .map_err(|_| WireError::new(ErrorCode::InvalidAutoTopupPolicy))?,
            amount: Microusd::new(
                i64::try_from(body.amount_cents.get())
                    .ok()
                    .and_then(|value| value.checked_mul(aex_finance_domain::MICROUSD_PER_CENT))
                    .ok_or_else(|| WireError::new(ErrorCode::InvalidAutoTopupPolicy))?,
            )
            .map_err(|_| WireError::new(ErrorCode::InvalidAutoTopupPolicy))?,
        };
        let record = self
            .authority
            .replace_policy(organization_id, change, current.revision)
            .await
            .map_err(authority_error)?;
        Ok(WithETag {
            etag: policy_etag(record.revision)?,
            value: policy_view(organization_id, &record)?,
        })
    }

    async fn billing_balance_get(
        &self,
        cx: &RequestContext,
        query: BillingBalanceGetQuery,
    ) -> WireResult<BillingBalance> {
        let organization = Self::selected(cx, &query)?;
        // Two point reads of one connection, concurrently: the money and the
        // state are separate authorities and neither is derived from the other,
        // so serialising them would buy nothing but latency.
        let (record, profile) = futures::try_join!(
            self.authority.balance(organization),
            self.authority.account_profile(organization),
        )
        .map_err(authority_error)?;
        Ok(BillingBalance {
            available_cents: cents(record.available)?,
            currency: Currency::USD,
            operational_state: operational_state(&profile)?,
            organization_id: organization,
            pending_cents: cents(record.pending)?,
            reserved_cents: cents(record.reserved)?,
            revision: record.revision,
            updated_at: instant(record.updated_at_millis)?,
        })
    }

    async fn billing_portal_session_create(
        &self,
        cx: &RequestContext,
        organization_id: OrganizationId,
        body: PortalSessionRequest,
    ) -> WireResult<Created<HostedSession>> {
        Self::authorize(cx, organization_id)?;
        let return_url = body
            .return_url
            .clone()
            .unwrap_or_else(|| self.default_return_url.clone());
        let now_millis = now_millis();
        let intent = format!(
            "portal:{}:{}",
            organization_id.encode().as_str(),
            return_url.as_str()
        );
        let session = self
            .hosted(
                organization_id,
                CommandKind::CreatePortalSession,
                intent.as_bytes(),
                None,
                move |effect, customer| PaymentCommand::CreatePortalSession {
                    effect,
                    organization: organization_id,
                    return_url,
                    customer,
                },
                now_millis,
            )
            .await?;
        Ok(Created(session))
    }

    async fn billing_statement_download_create(
        &self,
        cx: &RequestContext,
        organization_id: OrganizationId,
        statement_id: StatementId,
        _body: EmptyRequest,
    ) -> WireResult<Created<DownloadGrant>> {
        Self::authorize(cx, organization_id)?;
        let header = self
            .authority
            .statement(organization_id, statement_uuid(statement_id))
            .await
            .map_err(authority_error)?;
        let (Some(digest), Some(object_key)) =
            (header.content_sha256, header.object_key.as_deref())
        else {
            return Err(WireError::new(ErrorCode::NotFound));
        };
        let signed = self
            .downloads
            .presign(object_key, self.download_grant_ttl_ms)
            .await
            .map_err(|_| WireError::new(ErrorCode::NotFound))?;
        Ok(Created(DownloadGrant {
            authorized_bytes: DecimalU128::new(signed.size_bytes),
            expires_at: instant(signed.expires_at_millis)?,
            // One statement artifact is one measurable download resource, so the
            // measurement identity is derived from the statement rather than
            // minted per request; a replayed grant measures the same object.
            measurement_id: MeasurementId::from_uuid7(statement_id.uuid7()),
            range: None,
            sha256: aex_wire::ids::ContentHash::from_bytes(digest),
            size_bytes: DecimalU128::new(signed.size_bytes),
            url: signed.url,
        }))
    }

    async fn billing_statement_get(
        &self,
        cx: &RequestContext,
        organization_id: OrganizationId,
        statement_id: StatementId,
    ) -> WireResult<Statement> {
        Self::authorize(cx, organization_id)?;
        let header = self
            .authority
            .statement(organization_id, statement_uuid(statement_id))
            .await
            .map_err(authority_error)?;
        let lines = self
            .authority
            .statement_lines(organization_id, &header.period)
            .await
            .map_err(authority_error)?;
        let summary = summary(organization_id, &header)?;
        let mut rendered = Vec::with_capacity(lines.len());
        for line in lines {
            rendered.push(StatementLine {
                category: category(&line.category)?,
                total_cents: cents(line.total)?,
            });
        }
        Ok(Statement {
            artifact_hash: summary.artifact_hash,
            currency: Currency::USD,
            id: summary.id,
            issued_at: summary.issued_at,
            lines: rendered,
            organization_id,
            period: summary.period,
            total_cents: summary.total_cents,
        })
    }

    async fn billing_statements_list(
        &self,
        cx: &RequestContext,
        organization_id: OrganizationId,
        query: BillingStatementsListQuery,
    ) -> WireResult<StatementSummaryPage> {
        Self::authorize(cx, organization_id)?;
        let before = match query.cursor.as_ref() {
            None => None,
            Some(cursor) => Some(crate::download::decode_period_cursor(cursor)?),
        };
        let limit = query
            .limit
            .map_or(self.page_limit, |asked| asked.min(self.page_limit));
        let page = self
            .authority
            .statements(organization_id, before.as_deref(), limit)
            .await
            .map_err(authority_error)?;
        let mut items = Vec::with_capacity(page.items.len());
        for header in &page.items {
            items.push(summary(organization_id, header)?);
        }
        Ok(StatementSummaryPage {
            items,
            next_cursor: page
                .next_period
                .as_deref()
                .map(crate::download::encode_period_cursor)
                .transpose()?,
        })
    }

    async fn billing_top_up_checkout_create(
        &self,
        cx: &RequestContext,
        organization_id: OrganizationId,
        body: TopUpCheckoutRequest,
    ) -> WireResult<Created<HostedSession>> {
        Self::authorize(cx, organization_id)?;
        let amount = i64::try_from(body.amount_cents.get())
            .ok()
            .and_then(|value| value.checked_mul(aex_finance_domain::MICROUSD_PER_CENT))
            .and_then(|value| Microusd::new(value).ok())
            .ok_or_else(|| {
                WireError::new(ErrorCode::InvalidRequest)
                    .with_message("the requested top-up amount is outside the permitted bound")
            })?;
        let success_url = body
            .success_url
            .clone()
            .unwrap_or_else(|| self.default_return_url.clone());
        let cancel_url = body
            .cancel_url
            .clone()
            .unwrap_or_else(|| self.default_return_url.clone());
        let now_millis = now_millis();
        let intent = format!(
            "topup:{}:{}:{}:{}",
            organization_id.encode().as_str(),
            body.amount_cents.get(),
            success_url.as_str(),
            cancel_url.as_str()
        );
        let amount_cents = Cents::new(body.amount_cents.get());
        let session = self
            .hosted(
                organization_id,
                CommandKind::CreateTopUpCheckout,
                intent.as_bytes(),
                Some(amount),
                move |effect, customer| PaymentCommand::CreateTopUpCheckout {
                    effect,
                    organization: organization_id,
                    amount: amount_cents,
                    success_url,
                    cancel_url,
                    customer,
                    tax: TaxMode::ProviderAutomatic,
                },
                now_millis,
            )
            .await?;
        Ok(Created(session))
    }
}

/// The `uuid` payload of a statement identifier.
fn statement_uuid(statement_id: StatementId) -> uuid::Uuid {
    uuid::Uuid::from_bytes(*statement_id.uuid7().as_bytes())
}

/// The current instant in epoch milliseconds.
fn now_millis() -> i64 {
    let now = time::OffsetDateTime::now_utc();
    i64::try_from(now.unix_timestamp_nanos().div_euclid(1_000_000)).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use aex_control_domain::{AccountPauseCause, AccountProfile, AccountState};
    use aex_finance_domain::money::Microusd;
    use aex_wire::models::AccountOperationalState;
    use time::OffsetDateTime;

    use super::{category, cents, operational_state, period_range};

    fn profile(state: AccountState, reason: Option<&str>) -> AccountProfile {
        AccountProfile {
            state,
            reason: reason.map(str::to_owned),
            revision: 3,
            changed_at: OffsetDateTime::from_unix_timestamp(1_800_000_000).expect("an instant"),
        }
    }

    #[test]
    fn a_sub_cent_balance_is_refused_rather_than_rounded() {
        let exact = Microusd::new(20_000).expect("two cents");
        assert_eq!(cents(exact).expect("two cents").get(), 2);
        let remainder = Microusd::new(20_001).expect("two cents and a micro");
        assert!(
            cents(remainder).is_err(),
            "a public balance never rounds a remainder away"
        );
    }

    /// The projection is the only producer, so the balance beside it never
    /// moves the state — in either direction.
    ///
    /// Both directions matter. An account the projection calls active reads
    /// active at a zero balance, because the pause is the trigger's fact and
    /// finance must not front-run it; an account the projection calls paused
    /// reads paused while fully funded, because a dispute hold is not a
    /// shortage and a top-up would not clear it.
    #[test]
    fn the_published_state_is_the_projections_and_never_the_balances() {
        assert!(matches!(
            operational_state(&profile(AccountState::Active, None)).expect("active projects"),
            AccountOperationalState::Active(_)
        ));
        for cause in AccountPauseCause::ALL {
            assert!(
                matches!(
                    operational_state(&profile(
                        AccountState::PausedTopUpRequired,
                        Some(cause.as_str())
                    ))
                    .expect("a declared cause projects"),
                    AccountOperationalState::Paused(_)
                ),
                "{cause:?}"
            );
        }
        assert!(
            operational_state(&profile(AccountState::Unavailable, None)).is_err(),
            "an unestablished state is refused, never answered as active"
        );
    }

    #[test]
    fn a_period_is_a_half_open_calendar_month() {
        let range = period_range("2026-12").expect("December 2026");
        assert!(range.lt.unix_millis() > range.gte.unix_millis());
        let january = period_range("2027-01").expect("January 2027");
        assert_eq!(range.lt.unix_millis(), january.gte.unix_millis());
        assert!(period_range("2026-13").is_err());
    }

    /// The forward path `CommandKind::EnsureCustomer` never had.
    ///
    /// Every hosted command acts on a provider customer and nothing created one,
    /// so both routes refused every input. These two cases pin the shape that
    /// fixes it: the first hosted command of an organization's life creates the
    /// customer, and no later one creates a second.
    mod ensure_customer {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        use aex_control_domain::AccountProfile;
        use aex_finance_domain::money::Microusd;
        use aex_payment_contracts::{
            CommandKind, EffectId, EffectState, PaymentCommandEnvelope, PaymentResult,
            ProviderCustomerRef, ProviderObjectRef, RedactedEmail,
        };
        use aex_wire::ids::{OrganizationId, PrefixedId as _, Uuid7};
        use aex_wire::types::{Cents, HttpsUrl, Timestamp};

        use crate::authority::{
            AuthorityError, BalanceRecord, BillingAuthority, EffectPreparation, GatewayError,
            PaymentGateway, PolicyChange, PolicyRecord, StatementHeader, StatementLineRecord,
            StatementPage,
        };
        use crate::billing::BillingService;
        use crate::download::{DownloadError, PresignedObject, StatementDownloads};

        const CREATED: &str = "cus_created_by_the_forward_path";

        #[derive(Debug, Default)]
        struct FakeAuthority {
            customer: parking_lot::Mutex<Option<String>>,
            settled: AtomicUsize,
        }

        #[async_trait::async_trait]
        impl BillingAuthority for FakeAuthority {
            async fn provider_customer(
                &self,
                _organization: OrganizationId,
            ) -> Result<Option<ProviderCustomerRef>, AuthorityError> {
                Ok(self.customer.lock().clone().map(ProviderCustomerRef))
            }

            async fn billing_contact(
                &self,
                _organization: OrganizationId,
            ) -> Result<RedactedEmail, AuthorityError> {
                Ok(RedactedEmail::new("owner@example.test"))
            }

            async fn prepare_effect(
                &self,
                organization: OrganizationId,
                _kind: CommandKind,
                _intent: &[u8],
                _amount: Option<Microusd>,
                _deadline_millis: i64,
            ) -> Result<EffectPreparation, AuthorityError> {
                Ok(EffectPreparation {
                    effect: EffectId(effect_id()),
                    organization,
                    intent_hash: [7; 32],
                })
            }

            async fn bind_effect_command(
                &self,
                _effect: EffectId,
                _intent_hash: [u8; 32],
                _envelope: &PaymentCommandEnvelope,
            ) -> Result<(), AuthorityError> {
                Ok(())
            }

            async fn settle_customer(
                &self,
                _effect: EffectId,
                _organization: OrganizationId,
                result: &PaymentResult,
            ) -> Result<Option<ProviderCustomerRef>, AuthorityError> {
                self.settled.fetch_add(1, Ordering::SeqCst);
                if result.state() != EffectState::Succeeded {
                    return Ok(None);
                }
                let mut held = self.customer.lock();
                let effective = held.get_or_insert_with(|| CREATED.to_owned()).clone();
                Ok(Some(ProviderCustomerRef(effective)))
            }

            async fn probe_role(&self) -> Result<(), AuthorityError> {
                unimplemented!("not on the ensure-customer path")
            }
            async fn balance(
                &self,
                _organization: OrganizationId,
            ) -> Result<BalanceRecord, AuthorityError> {
                unimplemented!("not on the ensure-customer path")
            }
            async fn account_profile(
                &self,
                _organization: OrganizationId,
            ) -> Result<AccountProfile, AuthorityError> {
                unimplemented!("not on the ensure-customer path")
            }
            async fn policy(
                &self,
                _organization: OrganizationId,
            ) -> Result<PolicyRecord, AuthorityError> {
                unimplemented!("not on the ensure-customer path")
            }
            async fn replace_policy(
                &self,
                _organization: OrganizationId,
                _change: PolicyChange,
                _expect_revision: u64,
            ) -> Result<PolicyRecord, AuthorityError> {
                unimplemented!("not on the ensure-customer path")
            }
            async fn statements(
                &self,
                _organization: OrganizationId,
                _before_period: Option<&str>,
                _limit: u32,
            ) -> Result<StatementPage, AuthorityError> {
                unimplemented!("not on the ensure-customer path")
            }
            async fn statement(
                &self,
                _organization: OrganizationId,
                _statement_id: uuid::Uuid,
            ) -> Result<StatementHeader, AuthorityError> {
                unimplemented!("not on the ensure-customer path")
            }
            async fn statement_lines(
                &self,
                _organization: OrganizationId,
                _period: &str,
            ) -> Result<Vec<StatementLineRecord>, AuthorityError> {
                unimplemented!("not on the ensure-customer path")
            }
            async fn finalize_effect(
                &self,
                _effect: EffectId,
                _result: &PaymentResult,
            ) -> Result<(), AuthorityError> {
                unimplemented!("not on the ensure-customer path")
            }
        }

        /// A gateway that counts what it was asked to execute.
        #[derive(Debug, Default)]
        struct CountingGateway {
            calls: AtomicUsize,
        }

        #[async_trait::async_trait]
        impl PaymentGateway for CountingGateway {
            async fn execute(
                &self,
                envelope: &PaymentCommandEnvelope,
            ) -> Result<PaymentResult, GatewayError> {
                assert_eq!(
                    envelope.command.kind(),
                    CommandKind::EnsureCustomer,
                    "no other command reaches this suite"
                );
                self.calls.fetch_add(1, Ordering::SeqCst);
                Ok(PaymentResult::Succeeded {
                    effect: envelope.command.effect(),
                    provider_ref: ProviderObjectRef(CREATED.to_owned()),
                    provider_created_at: Timestamp::from_unix_millis(1_800_000_000_000)
                        .expect("an instant"),
                    hosted: None,
                    charged: Cents::ZERO,
                    tax: None,
                })
            }
        }

        #[derive(Debug)]
        struct UnreachableDownloads;

        #[async_trait::async_trait]
        impl StatementDownloads for UnreachableDownloads {
            async fn presign(
                &self,
                _object_key: &str,
                _ttl_millis: i64,
            ) -> Result<PresignedObject, DownloadError> {
                unimplemented!("not on the ensure-customer path")
            }
        }

        fn effect_id() -> Uuid7 {
            Uuid7::from_bytes(*uuid::Uuid::now_v7().as_bytes()).expect("a UUIDv7")
        }

        fn organization() -> OrganizationId {
            OrganizationId::from_uuid7(
                Uuid7::from_bytes([
                    0x01, 0x92, 0x3f, 0x2a, 0x7c, 0x00, 0x70, 0x00, 0x80, 0x00, 0, 0, 0, 0, 0, 2,
                ])
                .expect("a UUIDv7"),
            )
        }

        fn service(
            authority: Arc<FakeAuthority>,
            gateway: Arc<CountingGateway>,
        ) -> BillingService<FakeAuthority, CountingGateway, UnreachableDownloads> {
            BillingService::new(
                authority,
                gateway,
                Arc::new(UnreachableDownloads),
                25,
                60_000,
                HttpsUrl::parse("https://aex.test/billing").expect("a return URL"),
            )
        }

        #[tokio::test]
        async fn the_first_hosted_command_creates_the_provider_customer() {
            let authority = Arc::new(FakeAuthority::default());
            let gateway = Arc::new(CountingGateway::default());
            let service = service(Arc::clone(&authority), Arc::clone(&gateway));

            let customer = service
                .customer(organization(), 1_800_000_000_000)
                .await
                .expect("the customer is created on first use");

            assert_eq!(customer.0, CREATED);
            assert_eq!(gateway.calls.load(Ordering::SeqCst), 1);
            assert_eq!(
                authority.settled.load(Ordering::SeqCst),
                1,
                "the effect is closed and the column written in one settle"
            );
            assert_eq!(authority.customer.lock().as_deref(), Some(CREATED));
        }

        #[tokio::test]
        async fn an_organization_that_has_a_customer_never_creates_a_second() {
            let authority = Arc::new(FakeAuthority {
                customer: parking_lot::Mutex::new(Some("cus_existing".to_owned())),
                settled: AtomicUsize::new(0),
            });
            let gateway = Arc::new(CountingGateway::default());
            let service = service(Arc::clone(&authority), Arc::clone(&gateway));

            let customer = service
                .customer(organization(), 1_800_000_000_000)
                .await
                .expect("the existing customer resolves");

            assert_eq!(customer.0, "cus_existing");
            assert_eq!(
                gateway.calls.load(Ordering::SeqCst),
                0,
                "a second customer would orphan the first and every method saved on it"
            );
        }
    }

    #[test]
    fn only_the_four_meters_categories_are_admitted() {
        for spelling in ["storage", "compute", "memory", "transfer"] {
            assert!(category(spelling).is_ok(), "{spelling}");
        }
        assert!(category("tokens").is_err(), "there is no token category");
    }
}
