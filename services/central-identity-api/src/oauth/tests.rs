use super::{
    Endpoints, HandshakeError, HttpProviderHandshake, OauthClient, OauthClientError, challenge_of,
    form, google_profile, parse_google_token, redact, state_matches,
};
use aex_identity_domain::Provider;
use base64::Engine as _;
use std::time::Duration;
use time::OffsetDateTime;

fn client() -> OauthClient {
    OauthClient::new("goog-client", "goog-secret").expect("a client")
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
fn the_compiled_google_endpoint_pins_its_exact_https_authority_and_path() {
    let endpoint = Endpoints::default().google_token;
    let parsed = url::Url::parse(&endpoint).expect("the compiled endpoint parses");
    assert_eq!(parsed.scheme(), "https");
    assert_eq!(parsed.host_str(), Some("oauth2.googleapis.com"));
    assert_eq!(parsed.port(), None);
    assert_eq!(parsed.path(), "/token");
    assert_eq!(parsed.username(), "");
    assert_eq!(parsed.password(), None);
    assert_eq!(parsed.query(), None);
    assert_eq!(parsed.fragment(), None);
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
fn the_google_client_never_renders_its_secret() {
    let rendered = format!("{:?}", client());
    assert!(rendered.contains("goog-client"), "{rendered}");
    assert!(!rendered.contains("goog-secret"), "{rendered}");
    assert_eq!(client().id(), "goog-client");
}

#[test]
fn a_form_body_percent_encodes_every_value() {
    assert_eq!(
        form(&[("code", "a b&c"), ("redirect_uri", "https://x.dev/cb")]),
        "code=a+b%26c&redirect_uri=https%3A%2F%2Fx.dev%2Fcb"
    );
}

#[test]
fn provider_statuses_cannot_smuggle_a_token_past_their_meaning() {
    assert_eq!(
        parse_google_token(reqwest::StatusCode::TOO_MANY_REQUESTS, b"{}", "goog-secret")
            .expect_err("rate limited"),
        HandshakeError::RateLimited
    );
    for status in [
        reqwest::StatusCode::BAD_REQUEST,
        reqwest::StatusCode::UNAUTHORIZED,
        reqwest::StatusCode::FORBIDDEN,
    ] {
        assert!(matches!(
            parse_google_token(
                status,
                br#"{"id_token":"header.claims.signature"}"#,
                "goog-secret"
            )
            .expect_err("refused"),
            HandshakeError::Refused(_)
        ));
    }
    for status in [
        reqwest::StatusCode::INTERNAL_SERVER_ERROR,
        reqwest::StatusCode::BAD_GATEWAY,
        reqwest::StatusCode::SERVICE_UNAVAILABLE,
    ] {
        assert!(matches!(
            parse_google_token(
                status,
                br#"{"id_token":"header.claims.signature"}"#,
                "goog-secret"
            )
            .expect_err("unavailable"),
            HandshakeError::Unreachable(_)
        ));
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
fn an_invalid_avatar_is_dropped_and_an_invalid_name_never_blocks_sign_in() {
    let now = OffsetDateTime::now_utc();
    let mut claims = google_fixture(now);
    claims["picture"] = serde_json::json!("javascript:alert(1)");
    claims["name"] = serde_json::json!("x".repeat(129));
    let profile = google_profile(&id_token(&claims), "goog-client", now).expect("a person");
    assert_eq!(profile.image_url, None);
    assert_eq!(profile.name, None);
}

#[test]
fn a_diagnostic_never_carries_the_client_secret_or_a_credential_shaped_run() {
    let rendered = redact(
        "error connecting with client_secret=google-secret and token provider_0123456789abcdefghijklmnop",
        "google-secret",
    );
    assert!(!rendered.contains("google-secret"), "{rendered}");
    assert!(
        !rendered.contains("provider_0123456789abcdefghijklmnop"),
        "{rendered}"
    );
}

#[test]
fn the_pinned_client_policy_builds() {
    HttpProviderHandshake::new(
        client(),
        "https://dev.aex.dev/api/auth/callback".to_owned(),
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
