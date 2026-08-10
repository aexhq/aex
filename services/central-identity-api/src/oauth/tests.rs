use super::{
    Endpoints, GITHUB_API_VERSION, HandshakeError, HttpProviderHandshake, OauthClient,
    OauthClientError, OauthClients, challenge_of, form, github_profile, google_profile,
    parse_github_token, parse_google_token, redact, state_matches,
};
use aex_identity_domain::Provider;
use base64::Engine as _;
use std::time::Duration;
use time::OffsetDateTime;

fn clients() -> OauthClients {
    OauthClients::new(
        OauthClient::new("gh-client", "gh-secret").expect("a client"),
        OauthClient::new("goog-client", "goog-secret").expect("a client"),
    )
}

fn id_token(claims: &serde_json::Value) -> String {
    let encode = |value: &serde_json::Value| {
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(value).expect("serializable"))
    };
    format!(
        "{}.{}.{}",
        encode(&serde_json::json!({"alg": "RS256"})),
        encode(claims),
        "c2lnbmF0dXJl"
    )
}

fn google_fixture(now: OffsetDateTime) -> serde_json::Value {
    serde_json::json!({
        "iss": "https://accounts.google.com",
        "aud": "goog-client",
        "sub": "1234567890",
        "exp": now.unix_timestamp() + 300,
        "iat": now.unix_timestamp() - 5,
        "email": "Person@Example.COM",
        "email_verified": true,
        "name": "A Person",
        "picture": "https://example.com/a.png",
    })
}

#[test]
fn a_challenge_is_the_rfc_7636_s256_of_its_verifier() {
    assert_eq!(
        challenge_of("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
        "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
    );
}

#[test]
fn a_state_matches_only_the_verifier_it_was_derived_from() {
    let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    assert!(state_matches(verifier, &challenge_of(verifier)));
    for forged in [
        "",
        "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM ",
        "e9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
        &challenge_of("another-verifier-entirely-0000000000000000000"),
    ] {
        assert!(!state_matches(verifier, forged), "{forged}");
    }
}

#[test]
fn an_attackers_state_never_matches_a_victims_verifier() {
    let victim = "victim-verifier-0000000000000000000000000000";
    let attacker = "attacker-verifier-00000000000000000000000000";
    assert!(!state_matches(victim, &challenge_of(attacker)));
}

#[test]
fn every_compiled_provider_endpoint_pins_its_exact_https_authority_and_path() {
    let endpoints = Endpoints::default();
    let expected = [
        (
            endpoints.github_token.as_str(),
            "github.com",
            "/login/oauth/access_token",
        ),
        (endpoints.github_api.as_str(), "api.github.com", "/"),
        (
            endpoints.google_token.as_str(),
            "oauth2.googleapis.com",
            "/token",
        ),
    ];
    for (endpoint, host, path) in expected {
        let parsed = url::Url::parse(endpoint).expect("a compiled endpoint parses");
        assert_eq!(parsed.scheme(), "https", "{endpoint}");
        assert_eq!(parsed.host_str(), Some(host), "{endpoint}");
        assert_eq!(parsed.port(), None, "{endpoint} pins a non-default port");
        assert_eq!(parsed.path(), path, "{endpoint}");
        assert_eq!(parsed.username(), "", "{endpoint}");
        assert_eq!(parsed.password(), None, "{endpoint}");
        assert_eq!(parsed.query(), None, "{endpoint}");
        assert_eq!(parsed.fragment(), None, "{endpoint}");
    }
}

#[test]
fn a_client_with_a_blank_half_is_refused_at_construction() {
    assert_eq!(
        OauthClient::new(" ", "secret"),
        Err(OauthClientError::BlankId)
    );
    assert_eq!(
        OauthClient::new("id", "  "),
        Err(OauthClientError::BlankSecret)
    );
}

#[test]
fn a_client_never_renders_its_secret() {
    let rendered = format!("{:?}", clients());
    assert!(rendered.contains("gh-client"), "{rendered}");
    assert!(!rendered.contains("gh-secret"), "{rendered}");
    assert!(!rendered.contains("goog-secret"), "{rendered}");
}

#[test]
fn each_provider_exchanges_as_its_own_registered_client() {
    let clients = clients();
    assert_eq!(clients.of(Provider::Github).id(), "gh-client");
    assert_eq!(clients.of(Provider::Google).id(), "goog-client");
}

#[test]
fn a_form_body_percent_encodes_every_value() {
    assert_eq!(
        form(&[("code", "a b&c"), ("redirect_uri", "https://x.dev/cb")]),
        "code=a+b%26c&redirect_uri=https%3A%2F%2Fx.dev%2Fcb"
    );
}

#[test]
fn a_github_refusal_arrives_as_two_hundred_and_is_still_a_refusal() {
    let error = parse_github_token(
        reqwest::StatusCode::OK,
        br#"{"error":"bad_verification_code","error_description":"The code passed is incorrect or expired."}"#,
        "gh-secret",
    )
    .expect_err("a refusal");
    assert!(matches!(error, HandshakeError::Refused(_)), "{error}");
}

#[test]
fn a_github_token_is_read_from_a_successful_redemption() {
    let token = parse_github_token(
        reqwest::StatusCode::OK,
        br#"{"access_token":"gho_fixture","scope":"user:email","token_type":"bearer"}"#,
        "gh-secret",
    )
    .expect("a token");
    assert_eq!(token, "gho_fixture");
}

#[test]
fn a_provider_rate_limit_is_never_reported_as_a_refused_code() {
    for parsed in [
        parse_github_token(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            br#"{"access_token":"gho_must_not_escape"}"#,
            "gh-secret",
        ),
        parse_google_token(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            br#"{"id_token":"header.claims.signature"}"#,
            "goog-secret",
        ),
    ] {
        assert_eq!(parsed.expect_err("a refusal"), HandshakeError::RateLimited);
    }
}

#[test]
fn a_client_error_can_never_smuggle_a_token_past_its_status() {
    for status in [
        reqwest::StatusCode::BAD_REQUEST,
        reqwest::StatusCode::UNAUTHORIZED,
        reqwest::StatusCode::FORBIDDEN,
    ] {
        for parsed in [
            parse_github_token(
                status,
                br#"{"access_token":"gho_must_not_escape"}"#,
                "gh-secret",
            ),
            parse_google_token(
                status,
                br#"{"id_token":"header.claims.signature"}"#,
                "goog-secret",
            ),
        ] {
            assert!(
                matches!(parsed.expect_err("a refusal"), HandshakeError::Refused(_)),
                "{status}"
            );
        }
    }
}

#[test]
fn a_server_error_can_never_smuggle_a_token_past_its_status() {
    for status in [
        reqwest::StatusCode::INTERNAL_SERVER_ERROR,
        reqwest::StatusCode::BAD_GATEWAY,
        reqwest::StatusCode::SERVICE_UNAVAILABLE,
    ] {
        for parsed in [
            parse_github_token(
                status,
                br#"{"access_token":"gho_must_not_escape"}"#,
                "gh-secret",
            ),
            parse_google_token(
                status,
                br#"{"id_token":"header.claims.signature"}"#,
                "goog-secret",
            ),
        ] {
            assert!(
                matches!(
                    parsed.expect_err("an unavailable provider"),
                    HandshakeError::Unreachable(_)
                ),
                "{status}"
            );
        }
    }
}

#[test]
fn a_google_refusal_is_read_from_its_error_member() {
    let error = parse_google_token(
        reqwest::StatusCode::BAD_REQUEST,
        br#"{"error":"invalid_grant","error_description":"Bad Request"}"#,
        "goog-secret",
    )
    .expect_err("a refusal");
    assert!(matches!(error, HandshakeError::Refused(_)), "{error}");
}

#[test]
fn a_github_person_is_built_from_the_verified_primary_address() {
    let profile = github_profile(
        br#"{"id":42,"login":"person","name":"A Person","avatar_url":"https://example.com/a.png"}"#,
        br#"[{"email":"other@example.com","primary":false,"verified":true},
             {"email":"Person@Example.COM","primary":true,"verified":true}]"#,
    )
    .expect("a person");
    assert_eq!(profile.provider, Provider::Github);
    assert_eq!(profile.provider_account_id.as_str(), "42");
    assert_eq!(profile.email.as_str(), "person@example.com");
    assert_eq!(profile.name.as_deref(), Some("A Person"));
    assert_eq!(
        profile.image_url.as_deref(),
        Some("https://example.com/a.png")
    );
}

#[test]
fn github_without_a_verified_primary_address_yields_nobody() {
    for emails in [
        &br#"[{"email":"person@example.com","primary":true,"verified":false}]"#[..],
        &br#"[{"email":"person@example.com","primary":false,"verified":true}]"#[..],
        &b"[]"[..],
    ] {
        let error = github_profile(br#"{"id":42}"#, emails).expect_err("no person");
        assert!(matches!(error, HandshakeError::Unusable(_)), "{error}");
    }
}

#[test]
fn a_google_person_is_built_from_a_checked_id_token() {
    let now = OffsetDateTime::now_utc();
    let profile =
        google_profile(&id_token(&google_fixture(now)), "goog-client", now).expect("a person");
    assert_eq!(profile.provider, Provider::Google);
    assert_eq!(profile.provider_account_id.as_str(), "1234567890");
    assert_eq!(profile.email.as_str(), "person@example.com");
}

#[test]
fn an_id_token_for_another_client_is_refused() {
    let now = OffsetDateTime::now_utc();
    let error = google_profile(&id_token(&google_fixture(now)), "somebody-else", now)
        .expect_err("no person");
    assert!(matches!(error, HandshakeError::Unusable(_)), "{error}");
}

#[test]
fn every_claim_this_platform_acts_on_is_checked() {
    let now = OffsetDateTime::now_utc();
    for (field, value) in [
        ("iss", serde_json::json!("https://accounts.evil.example")),
        ("aud", serde_json::json!("another-client")),
        ("exp", serde_json::json!(now.unix_timestamp() - 1)),
        ("iat", serde_json::json!(now.unix_timestamp() + 3_600)),
        ("email_verified", serde_json::json!(false)),
        ("email", serde_json::Value::Null),
    ] {
        let mut claims = google_fixture(now);
        claims[field] = value;
        let error = google_profile(&id_token(&claims), "goog-client", now)
            .unwrap_err_or_else_message(field);
        assert!(matches!(error, HandshakeError::Unusable(_)), "{field}");
    }
}

#[test]
fn a_token_that_is_not_a_jwt_is_refused_before_any_claim_is_read() {
    let now = OffsetDateTime::now_utc();
    for forged in ["", "one.two", "one.two.three.four", "one.!!!.three"] {
        let error = google_profile(forged, "goog-client", now).expect_err(forged);
        assert!(matches!(error, HandshakeError::Unusable(_)), "{forged}");
    }
}

#[test]
fn a_non_https_avatar_is_dropped_rather_than_stored() {
    let profile = github_profile(
        br#"{"id":42,"avatar_url":"javascript:alert(1)"}"#,
        br#"[{"email":"person@example.com","primary":true,"verified":true}]"#,
    )
    .expect("a person");
    assert_eq!(profile.image_url, None);
}

#[test]
fn a_name_the_database_would_refuse_is_dropped_rather_than_failing_the_sign_in() {
    let long = "x".repeat(129);
    let profile = github_profile(
        format!(r#"{{"id":42,"name":"{long}"}}"#).as_bytes(),
        br#"[{"email":"person@example.com","primary":true,"verified":true}]"#,
    )
    .expect("a person");
    assert_eq!(profile.name, None);
}

#[test]
fn a_diagnostic_never_carries_the_client_secret_or_a_credential_shaped_run() {
    let rendered = redact(
        "error connecting with client_secret=gh-secret and token gho_0123456789abcdefghijklmnop",
        "gh-secret",
    );
    assert!(!rendered.contains("gh-secret"), "{rendered}");
    assert!(
        !rendered.contains("gho_0123456789abcdefghijklmnop"),
        "{rendered}"
    );
}

#[test]
fn the_github_api_version_is_pinned() {
    assert_eq!(GITHUB_API_VERSION, "2026-03-10");
}

#[test]
fn the_pinned_client_policy_builds() {
    HttpProviderHandshake::new(
        clients(),
        "https://dash.aex.dev/auth/callback".to_owned(),
        Duration::from_secs(5),
    )
    .expect("the pinned configuration builds");
}

trait ExpectErrNamed<T, E> {
    fn unwrap_err_or_else_message(self, field: &str) -> E;
}

impl<T, E> ExpectErrNamed<T, E> for Result<T, E> {
    fn unwrap_err_or_else_message(self, field: &str) -> E {
        match self {
            Ok(_) => panic!("`{field}` was accepted"),
            Err(error) => error,
        }
    }
}
