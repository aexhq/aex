use super::{
    Endpoints, HandshakeError, OauthClient, OauthClientError, challenge_of, github_profile,
    google_profile, redact, state_matches,
};
use aex_identity_domain::Provider;
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
    for (endpoint, host, path) in [
        (endpoints.google_token, "oauth2.googleapis.com", "/token"),
        (
            endpoints.github_token,
            "github.com",
            "/login/oauth/access_token",
        ),
        (endpoints.github_user, "api.github.com", "/user"),
        (endpoints.github_emails, "api.github.com", "/user/emails"),
    ] {
        let parsed = oauth2::url::Url::parse(&endpoint).expect("the compiled endpoint parses");
        assert_eq!(parsed.scheme(), "https");
        assert_eq!(parsed.host_str(), Some(host));
        assert_eq!(parsed.port(), None);
        assert_eq!(parsed.path(), path);
        assert!(parsed.query().is_none());
        assert!(parsed.fragment().is_none());
    }
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
    let client = OauthClient::new("github-client", "github-secret").expect("a client");
    let debug = format!("{client:?}");
    assert!(debug.contains("github-client"));
    assert!(!debug.contains("github-secret"));
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
fn github_uses_only_a_verified_primary_email_and_normalizes_it() {
    let user = serde_json::from_value(serde_json::json!({
        "id": 42,
        "login": "octocat",
        "name": "The Octocat",
        "avatar_url": "https://avatars.githubusercontent.com/u/42"
    }))
    .expect("documented GitHub user");
    let emails = serde_json::from_value(serde_json::json!([
        {"email": "unverified@example.com", "primary": false, "verified": false},
        {"email": "Person@Example.COM", "primary": true, "verified": true}
    ]))
    .expect("documented GitHub emails");
    let profile = github_profile(user, emails).expect("a verified identity");
    assert_eq!(profile.provider, Provider::GitHub);
    assert_eq!(profile.provider_account_id.as_str(), "42");
    assert_eq!(profile.email.as_str(), "person@example.com");
    assert_eq!(profile.name.as_deref(), Some("The Octocat"));
}

#[test]
fn github_refuses_an_unverified_or_non_primary_address() {
    for emails in [
        serde_json::json!([{"email": "x@example.com", "primary": true, "verified": false}]),
        serde_json::json!([{"email": "x@example.com", "primary": false, "verified": true}]),
        serde_json::json!([]),
    ] {
        let user = serde_json::from_value(serde_json::json!({
            "id": 42,
            "login": "octocat",
            "name": null,
            "avatar_url": null
        }))
        .expect("documented GitHub user");
        let emails = serde_json::from_value(emails).expect("documented GitHub emails");
        assert!(matches!(
            github_profile(user, emails),
            Err(HandshakeError::Unusable(_))
        ));
    }
}

#[test]
fn a_diagnostic_never_carries_a_client_secret_or_credential_shaped_run() {
    let rendered = redact(
        "error client_secret=github-secret token=cred_0123456789abcdefghijklmnop",
        "github-secret",
    );
    assert!(!rendered.contains("github-secret"), "{rendered}");
    assert!(
        !rendered.contains("cred_0123456789abcdefghijklmnop"),
        "{rendered}"
    );
}
