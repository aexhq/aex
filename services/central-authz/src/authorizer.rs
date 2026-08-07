//! API Gateway REQUEST-authorizer input, credential resolution and output.
//!
//! The public edge sends a bearer credential to API Gateway. This module turns
//! the AWS-owned event into the same typed context every central Rust edge
//! consumes. A malformed or inactive credential is one undifferentiated
//! `Unauthorized` result; an unreadable authority or an unknown pepper is a
//! Lambda fault, because claiming the credential is invalid when it could not
//! be checked would be false.

use aex_central_http::authorizer::{
    CentralAuthorizerContext, ContextPrincipalKind, MAX_CONTEXT_LIFETIME_MS,
};
use aex_control_app::ports::{AuthorizationReader, CentralActorState};
use aex_control_domain::{AccountState, OrganizationStatus, WorkspaceStatus};
use aex_identity_domain::credential::{
    CredentialKind, ParsedCredential, PepperVersion, Verifier, parse, verify,
};
use aex_wire::types::RequestId;
use aws_lambda_events::apigw::{
    ApiGatewayCustomAuthorizerRequestTypeRequest, ApiGatewayV2CustomAuthorizerV2Request,
};
use http::HeaderMap;
use time::OffsetDateTime;

use crate::Config;
use crate::issue::PepperRing;

/// One normalized API Gateway authorizer invocation.
#[derive(Clone)]
pub struct RequestInvocation {
    version: PayloadVersion,
    credential: ParsedCredential,
    request_id: RequestId,
    resource_arn: ResourceArn,
}

impl std::fmt::Debug for RequestInvocation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RequestInvocation")
            .field("version", &self.version)
            .field("credential_kind", &self.credential.kind)
            .field("credential_id", &self.credential.id)
            .field("request_id", &self.request_id)
            .field("resource_arn", &self.resource_arn)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PayloadVersion {
    V1,
    V2,
}

/// An API Gateway execute-api ARN already checked against this deployment.
#[derive(Debug, Clone)]
struct ResourceArn(String);

/// Why an authorizer invocation could not be answered truthfully.
#[derive(Debug, thiserror::Error)]
pub enum AuthorizerFault {
    /// API Gateway sent an event outside the contract this binary serves.
    #[error("the API Gateway authorizer event is malformed: {0}")]
    Malformed(String),
    /// The read-only authorization store did not answer.
    #[error("the authorization store could not answer: {0}")]
    Store(String),
    /// A stored verifier names a pepper this cold start did not load.
    #[error("pepper version {0} is not held by this process")]
    UnknownPepper(u16),
}

impl RequestInvocation {
    /// Whether a payload names the API Gateway operation.
    #[must_use]
    pub fn matches(payload: &serde_json::Value) -> bool {
        payload.get("type").and_then(serde_json::Value::as_str) == Some("REQUEST")
    }

    /// Decodes and validates either the HTTP API v2 event or the v1 IAM-policy
    /// event.
    ///
    /// # Errors
    ///
    /// Returns [`AuthorizerFault::Malformed`] when an AWS-owned field is absent,
    /// a bearer value is ambiguous, or the invoke ARN is outside this plane.
    pub fn parse(
        payload: &serde_json::Value,
        config: &Config,
    ) -> Result<Option<Self>, AuthorizerFault> {
        if payload.get("version").and_then(serde_json::Value::as_str) == Some("2.0") {
            let event: ApiGatewayV2CustomAuthorizerV2Request =
                serde_json::from_value(payload.clone()).map_err(malformed)?;
            if event.type_.as_deref() != Some("REQUEST") {
                return Err(malformed("the type is not REQUEST"));
            }
            let arn = event
                .route_arn
                .as_deref()
                .ok_or_else(|| malformed("routeArn is absent"))?;
            let request_id = event
                .request_context
                .request_id
                .as_deref()
                .ok_or_else(|| malformed("requestContext.requestId is absent"))?;
            return Self::new(
                PayloadVersion::V2,
                &event.headers,
                None,
                request_id,
                arn,
                config,
            );
        }

        let event: ApiGatewayCustomAuthorizerRequestTypeRequest =
            serde_json::from_value(payload.clone()).map_err(malformed)?;
        if event.type_.as_deref() != Some("REQUEST") {
            return Err(malformed("the type is not REQUEST"));
        }
        let arn = event
            .method_arn
            .as_deref()
            .ok_or_else(|| malformed("methodArn is absent"))?;
        let request_id = event
            .request_context
            .request_id
            .as_deref()
            .ok_or_else(|| malformed("requestContext.requestId is absent"))?;
        Self::new(
            PayloadVersion::V1,
            &event.headers,
            Some(&event.multi_value_headers),
            request_id,
            arn,
            config,
        )
    }

    fn new(
        version: PayloadVersion,
        headers: &HeaderMap,
        multi_value_headers: Option<&HeaderMap>,
        request_id: &str,
        resource_arn: &str,
        config: &Config,
    ) -> Result<Option<Self>, AuthorizerFault> {
        let request_id = RequestId::parse(request_id)
            .map_err(|_| malformed("requestContext.requestId is invalid"))?;
        let resource_arn = validate_arn(resource_arn, config)?;
        let Some(raw) = bearer(headers, multi_value_headers) else {
            return Ok(None);
        };
        let Some(credential) = parse_supported(raw) else {
            return Ok(None);
        };
        Ok(Some(Self {
            version,
            credential,
            request_id,
            resource_arn,
        }))
    }
}

fn malformed(reason: impl std::fmt::Display) -> AuthorizerFault {
    AuthorizerFault::Malformed(reason.to_string())
}

fn bearer<'a>(headers: &'a HeaderMap, multi: Option<&'a HeaderMap>) -> Option<&'a str> {
    let from_multi = multi.and_then(|all| {
        let mut values = all.get_all(http::header::AUTHORIZATION).iter();
        let one = values.next()?;
        values.next().is_none().then_some(one)
    });
    let value = if let Some(value) = from_multi {
        if let Some(single) = headers.get(http::header::AUTHORIZATION)
            && single != value
        {
            return None;
        }
        value
    } else {
        let mut values = headers.get_all(http::header::AUTHORIZATION).iter();
        let one = values.next()?;
        if values.next().is_some() {
            return None;
        }
        one
    };
    let value = value.to_str().ok()?;
    if value.contains(',') {
        return None;
    }
    let (scheme, credential) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("Bearer")
        || credential.is_empty()
        || credential.bytes().any(|byte| byte.is_ascii_whitespace())
    {
        return None;
    }
    Some(credential)
}

fn parse_supported(raw: &str) -> Option<ParsedCredential> {
    let kind = if raw.starts_with(CredentialKind::WorkspaceKey.prefix()) {
        CredentialKind::WorkspaceKey
    } else if raw.starts_with(CredentialKind::AccountToken.prefix()) {
        CredentialKind::AccountToken
    } else if raw.starts_with(CredentialKind::DashboardSession.prefix()) {
        CredentialKind::DashboardSession
    } else {
        return None;
    };
    parse(kind, raw).ok()
}

fn validate_arn(arn: &str, config: &Config) -> Result<ResourceArn, AuthorizerFault> {
    let mut fields = arn.splitn(6, ':');
    let (
        Some("arn"),
        Some(partition),
        Some("execute-api"),
        Some(region),
        Some(account),
        Some(path),
    ) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    )
    else {
        return Err(malformed("the execute-api ARN has the wrong shape"));
    };
    let segments: Vec<&str> = path.split('/').collect();
    if segments.len() < 3 || segments.iter().any(|segment| segment.is_empty()) {
        return Err(malformed("the execute-api ARN has no API, stage, or route"));
    }
    let [api, stage, method, ..] = segments.as_slice() else {
        return Err(malformed("the execute-api ARN has no API, stage, or route"));
    };
    if partition.is_empty()
        || api.is_empty()
        || stage.is_empty()
        || method.is_empty()
        || region != config.region.as_str()
        || account != config.account_id
    {
        return Err(malformed("the execute-api ARN is outside this deployment"));
    }
    Ok(ResourceArn(arn.to_owned()))
}

/// Resolves a request into a successful authorizer response.
///
/// `Ok(None)` is the one public refusal and becomes API Gateway's 401. Store and
/// pepper failures remain errors so no outage is misreported as a bad token.
pub async fn answer<R: AuthorizationReader>(
    reader: &R,
    peppers: &PepperRing,
    config: &Config,
    request: &RequestInvocation,
    now: OffsetDateTime,
) -> Result<Option<serde_json::Value>, AuthorizerFault> {
    let context = match request.credential.kind {
        CredentialKind::WorkspaceKey => {
            let Some(state) = reader
                .resolve_workspace_key(request.credential.id)
                .await
                .map_err(|error| AuthorizerFault::Store(error.to_string()))?
            else {
                return Ok(None);
            };
            // The token names its own region and workspace. Both are compared
            // against the key's row rather than trusted: a token whose pin
            // disagrees with the key it names is refused outright, so the
            // segments a regional edge reads to address its rows can never
            // describe a key that lives somewhere else.
            let pin = request.credential.pin;
            if state.key_id != request.credential.id
                || pin.map(|pin| pin.region.region()) != Some(state.region)
                || pin.map(|pin| pin.workspace) != Some(state.workspace_id)
                || state.key_revoked
                || state.workspace_status != WorkspaceStatus::Active
                || state.organization_status != OrganizationStatus::Active
                || !credential_matches(
                    peppers,
                    state.pepper_version,
                    &request.credential,
                    state.verifier,
                )?
            {
                return Ok(None);
            }
            context_for_key(request, &state, now, config.assertion_ttl_ms)
        }
        CredentialKind::AccountToken | CredentialKind::DashboardSession => {
            let state = match request.credential.kind {
                CredentialKind::AccountToken => {
                    reader
                        .resolve_account_token_central(request.credential.id, now)
                        .await
                }
                CredentialKind::DashboardSession => {
                    reader
                        .resolve_dashboard_session_central(request.credential.id, now)
                        .await
                }
                _ => unreachable!("the match arm admits only actor credentials"),
            }
            .map_err(|error| AuthorizerFault::Store(error.to_string()))?;
            let Some(state) = state else {
                return Ok(None);
            };
            if state.credential_id != request.credential.id
                || state.credential_revoked
                || state.credential_expired
                || !state.user_active
                || !credential_matches(
                    peppers,
                    state.pepper_version,
                    &request.credential,
                    state.verifier,
                )?
            {
                return Ok(None);
            }
            context_for_actor(request, state, now, config.assertion_ttl_ms)
        }
        CredentialKind::EmailChallenge | CredentialKind::DeviceCode => return Ok(None),
    };
    let principal_id = context.principal_id.to_string();
    let fields = context.to_fields();
    let response = match request.version {
        PayloadVersion::V2 => serde_json::json!({
            "isAuthorized": true,
            "context": fields,
        }),
        PayloadVersion::V1 => serde_json::json!({
            "principalId": principal_id,
            "policyDocument": {
                "Version": "2012-10-17",
                "Statement": [{
                    "Action": "execute-api:Invoke",
                    "Effect": "Allow",
                    "Resource": resource_arn(request),
                }],
            },
            "context": fields,
        }),
    };
    Ok(Some(response))
}

fn credential_matches(
    peppers: &PepperRing,
    version: u16,
    credential: &ParsedCredential,
    stored: [u8; 32],
) -> Result<bool, AuthorizerFault> {
    let pepper = peppers
        .get(PepperVersion::new(version))
        .ok_or(AuthorizerFault::UnknownPepper(version))?;
    Ok(verify(
        pepper,
        &credential.digest,
        &Verifier::from_bytes(stored),
    ))
}

fn window(now: OffsetDateTime, ttl_ms: u64) -> (i64, i64) {
    let issued_at_ms =
        i64::try_from(now.unix_timestamp_nanos().div_euclid(1_000_000)).unwrap_or(i64::MAX);
    let bounded = i64::try_from(ttl_ms)
        .unwrap_or(MAX_CONTEXT_LIFETIME_MS)
        .min(MAX_CONTEXT_LIFETIME_MS);
    (issued_at_ms, issued_at_ms.saturating_add(bounded))
}

fn context_for_key(
    request: &RequestInvocation,
    state: &aex_control_app::ports::WorkspaceKeyState,
    now: OffsetDateTime,
    ttl_ms: u64,
) -> CentralAuthorizerContext {
    let (issued_at_ms, expires_at_ms) = window(now, ttl_ms);
    CentralAuthorizerContext {
        request_id: request.request_id.clone(),
        kind: ContextPrincipalKind::WorkspaceKey,
        principal_id: state.key_id,
        credential_id: Some(state.key_id),
        workspace_id: Some(state.workspace_id),
        organization_id: Some(state.organization_id),
        region: Some(state.region),
        memberships: Vec::new(),
        scopes: state.scopes,
        account_state: state.account_state,
        issued_at_ms,
        expires_at_ms,
    }
}

fn context_for_actor(
    request: &RequestInvocation,
    state: CentralActorState,
    now: OffsetDateTime,
    ttl_ms: u64,
) -> CentralAuthorizerContext {
    let (issued_at_ms, expires_at_ms) = window(now, ttl_ms);
    CentralAuthorizerContext {
        request_id: request.request_id.clone(),
        kind: match request.credential.kind {
            CredentialKind::AccountToken => ContextPrincipalKind::Account,
            CredentialKind::DashboardSession => ContextPrincipalKind::UserSession,
            _ => unreachable!("actor context requires an actor credential"),
        },
        principal_id: state.user_id,
        credential_id: Some(state.credential_id),
        workspace_id: None,
        organization_id: None,
        region: None,
        memberships: state.memberships,
        scopes: state.scopes,
        // One actor can belong to several organizations. Claiming one account
        // state here would be false; central admission reads the selected
        // organization's current state after authorization.
        account_state: AccountState::Unavailable,
        issued_at_ms,
        expires_at_ms,
    }
}

fn resource_arn(request: &RequestInvocation) -> &str {
    &request.resource_arn.0
}

#[cfg(test)]
mod tests {
    use super::{AuthorizerFault, RequestInvocation, answer};
    use crate::Config;
    use crate::issue::parse_pepper_ring;
    use aex_central_http::CentralAuthorizerContext;
    use aex_central_http::config::DeploymentPlane;
    use aex_control_app::ports::{
        AccountActorState, AuthorizationReader, CentralActorState, SigningKeyRecord, StoreError,
        WorkspaceKeyState,
    };
    use aex_control_domain::{AccountState, Epoch, OrganizationStatus, ScopeSet, WorkspaceStatus};
    use aex_identity_domain::credential::{
        CredentialKind, Pepper, RegionCode, SecretRng, WorkspacePin, mint, verifier,
    };
    use aex_wire::types::Region;
    use async_trait::async_trait;
    use std::collections::BTreeMap;
    use time::OffsetDateTime;
    use uuid::Uuid;

    const PEPPER_BYTES: [u8; 32] = [17; 32];

    #[derive(Debug)]
    struct FixedRng;

    impl SecretRng for FixedRng {
        fn fill(&self, out: &mut [u8]) {
            out.fill(23);
        }
    }

    #[derive(Debug, Default)]
    struct Reader {
        actor: Option<CentralActorState>,
        key: Option<WorkspaceKeyState>,
    }

    #[async_trait]
    impl AuthorizationReader for Reader {
        async fn resolve_workspace_key(
            &self,
            _key_id: Uuid,
        ) -> Result<Option<WorkspaceKeyState>, StoreError> {
            Ok(self.key.clone())
        }

        async fn resolve_account_token_for_workspace(
            &self,
            _token_id: Uuid,
            _workspace_id: Uuid,
            _now: OffsetDateTime,
        ) -> Result<Option<AccountActorState>, StoreError> {
            Ok(None)
        }

        async fn resolve_session_for_workspace(
            &self,
            _session_id: Uuid,
            _workspace_id: Uuid,
            _now: OffsetDateTime,
        ) -> Result<Option<AccountActorState>, StoreError> {
            Ok(None)
        }

        async fn resolve_account_token_central(
            &self,
            _token_id: Uuid,
            _now: OffsetDateTime,
        ) -> Result<Option<CentralActorState>, StoreError> {
            Ok(self.actor.clone())
        }

        async fn resolve_dashboard_session_central(
            &self,
            _session_id: Uuid,
            _now: OffsetDateTime,
        ) -> Result<Option<CentralActorState>, StoreError> {
            Ok(self.actor.clone())
        }

        async fn verification_key_set(&self) -> Result<Vec<SigningKeyRecord>, StoreError> {
            Ok(Vec::new())
        }

        async fn active_signing_key(&self) -> Result<SigningKeyRecord, StoreError> {
            Err(StoreError::NotFound)
        }
    }

    fn config() -> Config {
        Config {
            plane: DeploymentPlane::Dev,
            region: Region::EuWest1,
            account_id: "000000000000".to_owned(),
            aurora_cluster_arn: "arn:aws:rds:eu-west-1:000000000000:cluster:aex".to_owned(),
            aurora_secret_arn: "arn:aws:secretsmanager:eu-west-1:000000000000:secret:aex"
                .to_owned(),
            signing_secret_id: "aex/dev/signing".to_owned(),
            pepper_secret_id: "aex/dev/pepper".to_owned(),
            database: "aex".to_owned(),
            role: "aex_authz".to_owned(),
            assertion_ttl_ms: 30_000,
        }
    }

    fn ring() -> crate::issue::PepperRing {
        let material = aex_control_domain::codec::base64url(&PEPPER_BYTES);
        parse_pepper_ring(
            "test",
            &format!(
                r#"{{"schemaVersion":1,"peppers":[{{"version":1,"material":"{material}"}}]}}"#
            ),
        )
        .expect("a pepper ring")
    }

    fn event(token: &str) -> serde_json::Value {
        serde_json::json!({
            "version": "2.0",
            "type": "REQUEST",
            "routeArn": "arn:aws:execute-api:eu-west-1:000000000000:api/$default/GET/api/workspaces",
            "identitySource": [format!("Bearer {token}")],
            "routeKey": "GET /api/workspaces",
            "rawPath": "/api/workspaces",
            "rawQueryString": "",
            "cookies": [],
            "headers": { "authorization": format!("Bearer {token}") },
            "requestContext": {
                "routeKey": "GET /api/workspaces",
                "accountId": "000000000000",
                "stage": "$default",
                "requestId": "gateway-request-1",
                "apiId": "api",
                "domainName": "api.example.invalid",
                "domainPrefix": "api",
                "time": "01/Jan/2026:00:00:00 +0000",
                "timeEpoch": 1_767_225_600_000_i64,
                "http": {
                    "method": "GET",
                    "path": "/api/workspaces",
                    "protocol": "HTTP/1.1",
                    "sourceIp": "127.0.0.1",
                    "userAgent": "test"
                }
            },
            "queryStringParameters": {},
            "pathParameters": {},
            "stageVariables": {}
        })
    }

    fn invocation(token: &str) -> RequestInvocation {
        RequestInvocation::parse(&event(token), &config())
            .expect("the event parses")
            .expect("the credential parses")
    }

    fn actor(
        token_id: Uuid,
        digest: &aex_identity_domain::credential::PresentedDigest,
    ) -> CentralActorState {
        CentralActorState {
            credential_id: token_id,
            user_id: Uuid::from_u128(0x22),
            scopes: ScopeSet::ALL,
            verifier: *verifier(&Pepper::new(PEPPER_BYTES), digest).as_bytes(),
            pepper_version: 1,
            credential_revoked: false,
            credential_expired: false,
            user_active: true,
            memberships: Vec::new(),
        }
    }

    fn response_context(response: &serde_json::Value) -> CentralAuthorizerContext {
        let fields: BTreeMap<String, String> = response["context"]
            .as_object()
            .expect("a context")
            .iter()
            .map(|(key, value)| {
                (
                    key.clone(),
                    value.as_str().expect("a string value").to_owned(),
                )
            })
            .collect();
        CentralAuthorizerContext::parse(&fields).expect("the edge accepts the context")
    }

    #[tokio::test]
    async fn an_account_token_produces_the_exact_context_the_edge_accepts() {
        let id = Uuid::from_u128(0x11);
        let (secret, digest) = mint(CredentialKind::AccountToken, None, id, &FixedRng);
        let request = invocation(secret.expose());
        let response = answer(
            &Reader {
                actor: Some(actor(id, &digest)),
                key: None,
            },
            &ring(),
            &config(),
            &request,
            OffsetDateTime::UNIX_EPOCH,
        )
        .await
        .expect("the authority answers")
        .expect("authorized");
        assert_eq!(response["isAuthorized"], true);
        let context = response_context(&response);
        assert_eq!(context.principal_id, Uuid::from_u128(0x22));
        assert_eq!(context.credential_id, Some(id));
        assert_eq!(context.account_state, AccountState::Unavailable);
        assert_eq!(context.expires_at_ms - context.issued_at_ms, 30_000);
    }

    #[tokio::test]
    async fn a_wrong_secret_is_unauthorized_without_revealing_which_check_failed() {
        let id = Uuid::from_u128(0x11);
        let (secret, digest) = mint(CredentialKind::AccountToken, None, id, &FixedRng);
        let mut state = actor(id, &digest);
        state.verifier = [99; 32];
        assert!(
            answer(
                &Reader {
                    actor: Some(state),
                    key: None,
                },
                &ring(),
                &config(),
                &invocation(secret.expose()),
                OffsetDateTime::UNIX_EPOCH,
            )
            .await
            .expect("the authority answers")
            .is_none()
        );
    }

    #[tokio::test]
    async fn a_workspace_key_preserves_placement_and_unavailable_account_state() {
        let id = Uuid::from_u128(0x31);
        let (secret, digest) = mint(
            CredentialKind::WorkspaceKey,
            Some(WorkspacePin {
                region: RegionCode::new(Region::EuWest1),
                workspace: Uuid::from_u128(0x32),
            }),
            id,
            &FixedRng,
        );
        let response = answer(
            &Reader {
                actor: None,
                key: Some(WorkspaceKeyState {
                    key_id: id,
                    workspace_id: Uuid::from_u128(0x32),
                    organization_id: Uuid::from_u128(0x33),
                    scopes: ScopeSet::ALL,
                    verifier: *verifier(&Pepper::new(PEPPER_BYTES), &digest).as_bytes(),
                    pepper_version: 1,
                    key_revoked: false,
                    region: Region::EuWest1,
                    workspace_status: WorkspaceStatus::Active,
                    organization_status: OrganizationStatus::Active,
                    account_state: AccountState::Unavailable,
                    epoch_key: Epoch::NEVER,
                    epoch_workspace: Epoch::NEVER,
                    epoch_account: Epoch::NEVER,
                }),
            },
            &ring(),
            &config(),
            &invocation(secret.expose()),
            OffsetDateTime::UNIX_EPOCH,
        )
        .await
        .expect("the authority answers")
        .expect("authorized");
        let context = response_context(&response);
        assert_eq!(context.workspace_id, Some(Uuid::from_u128(0x32)));
        assert_eq!(context.organization_id, Some(Uuid::from_u128(0x33)));
        assert_eq!(context.region, Some(Region::EuWest1));
        assert_eq!(context.account_state, AccountState::Unavailable);
    }

    #[test]
    fn malformed_credentials_are_unauthorized_but_foreign_arns_are_faults() {
        assert!(
            RequestInvocation::parse(&event("not-a-token"), &config())
                .expect("the AWS event is valid")
                .is_none()
        );
        let mut foreign = event("not-a-token");
        foreign["routeArn"] = serde_json::Value::String(
            "arn:aws:execute-api:us-east-1:000000000000:api/$default/GET/api/workspaces".to_owned(),
        );
        assert!(matches!(
            RequestInvocation::parse(&foreign, &config()),
            Err(AuthorizerFault::Malformed(_))
        ));
    }
}
