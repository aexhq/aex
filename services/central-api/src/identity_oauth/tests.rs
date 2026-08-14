use super::{
    CLI_REDIRECT_URI, Endpoints, HandshakeError, HttpProviderHandshake, OauthClient,
    OauthClientError, ProviderHandshake, challenge_of, google_profile, redact, state_matches,
};
use aex_identity_domain::Provider;
use aex_wire::models::AuthClient;
use base64::Engine as _;
use time::OffsetDateTime;

const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";

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
        "nonce": challenge_of(VERIFIER),
        "email": "Person@Example.COM",
        "email_verified": true,
        "name": "A Person",
        "picture": "https://example.com/a.png"
    })
}

#[test]
fn a_challenge_is_the_rfc_7636_s256_of_its_verifier() {
    assert_eq!(
        challenge_of(VERIFIER),
        "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
    );
}

#[test]
fn a_state_matches_only_the_verifier_it_was_derived_from() {
    assert!(state_matches(VERIFIER, &challenge_of(VERIFIER)));
    for forged in [
        "",
        "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM ",
        "e9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
        &challenge_of("another-verifier-entirely-0000000000000000000"),
    ] {
        assert!(!state_matches(VERIFIER, forged), "{forged}");
    }
}

#[test]
fn compiled_provider_endpoints_pin_their_exact_https_authorities_and_paths() {
    let endpoints = Endpoints::default();
    let parsed = oauth2::url::Url::parse(&endpoints.google_token)
        .expect("the compiled Google token endpoint parses");
    assert_eq!(parsed.scheme(), "https");
    assert_eq!(parsed.host_str(), Some("oauth2.googleapis.com"));
    assert_eq!(parsed.port(), None);
    assert_eq!(parsed.path(), "/token");
    assert!(parsed.query().is_none());
    assert!(parsed.fragment().is_none());
}

#[test]
fn oauth_clients_refuse_blank_halves_and_never_debug_the_secret() {
    assert_eq!(
        OauthClient::new("", "secret"),
        Err(OauthClientError::BlankId)
    );
    assert_eq!(
        OauthClient::new("id", " "),
        Err(OauthClientError::BlankSecret)
    );
    let client = OauthClient::new("google-client", "google-secret").expect("a client");
    let debug = format!("{client:?}");
    assert!(debug.contains("google-client"));
    assert!(!debug.contains("google-secret"));
}

#[test]
fn the_client_discriminator_selects_only_compiled_callbacks() {
    let handshake = HttpProviderHandshake::new(
        OauthClient::new("google-client", "google-secret").expect("client"),
        "https://dev.aex.dev/api/auth/callback".to_owned(),
        std::time::Duration::from_secs(10),
    )
    .expect("handshake");
    assert_eq!(handshake.public_client_id(), "google-client");
    assert_eq!(
        handshake.redirect_uri(AuthClient::Dashboard).as_str(),
        "https://dev.aex.dev/api/auth/callback"
    );
    assert_eq!(
        handshake.redirect_uri(AuthClient::Cli).as_str(),
        CLI_REDIRECT_URI
    );
}

#[test]
fn google_claim_mismatches_and_unverified_addresses_are_refused() {
    let now = OffsetDateTime::now_utc();
    for (field, value) in [
        ("iss", serde_json::json!("https://accounts.evil.example")),
        ("aud", serde_json::json!("another-client")),
        ("exp", serde_json::json!(now.unix_timestamp() - 1)),
        ("iat", serde_json::json!(now.unix_timestamp() + 3_600)),
        ("nonce", serde_json::json!("another-sign-in")),
        ("email_verified", serde_json::json!(false)),
        ("email", serde_json::Value::Null),
    ] {
        let mut claims = google_fixture(now);
        claims[field] = value;
        assert!(
            matches!(
                google_profile(
                    &id_token(&claims),
                    "goog-client",
                    &challenge_of(VERIFIER),
                    now
                ),
                Err(HandshakeError::Unusable(_))
            ),
            "{field}"
        );
    }
}

#[test]
fn google_normalizes_a_verified_identity() {
    let now = OffsetDateTime::now_utc();
    let profile = google_profile(
        &id_token(&google_fixture(now)),
        "goog-client",
        &challenge_of(VERIFIER),
        now,
    )
    .expect("a verified identity");
    assert_eq!(profile.provider, Provider::Google);
    assert_eq!(profile.provider_account_id.as_str(), "1234567890");
    assert_eq!(profile.email.as_str(), "person@example.com");
}

#[test]
fn a_diagnostic_never_carries_a_client_secret_or_credential_shaped_run() {
    let rendered = redact(
        "error client_secret=google-secret token=cred_0123456789abcdefghijklmnop",
        "google-secret",
    );
    assert!(!rendered.contains("google-secret"), "{rendered}");
    assert!(
        !rendered.contains("cred_0123456789abcdefghijklmnop"),
        "{rendered}"
    );
}
