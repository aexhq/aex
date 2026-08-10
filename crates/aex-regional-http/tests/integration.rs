//! The two cold-start readers, against `moto`'s Secrets Manager and SSM.
//!
//! What this proves: that the two documents a regional edge cannot start without
//! round-trip through a service rather than a fake, and that each one arrives
//! from the store it is actually provisioned in. A Secrets Manager id goes where
//! `GetSecretValue` expects it and comes back as a ring that admits a real
//! credential; a hierarchy name goes where `GetParameter` expects it, the
//! `WithDecryption` this reader always sets is accepted on a `String` and on a
//! `SecureString` alike, and a ring read out of the service really signs and
//! verifies a cursor. The two failure arms a service owns — an absent reference
//! and a blank value — arrive as `TrustError::Unreadable` and `TrustError::Empty`
//! rather than as an empty ring. The scripted `authz` target already fixes what
//! these documents *mean*; only a service fixes that the bytes arrive at all,
//! which is the half a composition root depends on before its listener binds.
//!
//! The pepper is read from Secrets Manager and the cursor ring from Parameter
//! Store, which is the split the deployed planes use: the pepper has a second
//! reader in the central plane and therefore exactly one home, and the cursor
//! ring has one reader and stays where it is.
//!
//! What it does not prove is stated rather than assumed. `moto` has no KMS
//! behind a `SecureString` or behind a secret's envelope — `release/policy/test-images.toml`
//! lists exactly that under `cannot_prove` — so the keys these documents are
//! really sealed under, their key policies and their grants stay live concerns,
//! and a green run here says nothing about a role that can read a reference but
//! not decrypt it. `moto` evaluates no IAM either, so the scoping that holds a
//! regional execution role to one secret and its own plane's parameter prefix is
//! untested: every read below succeeds for reasons a deployed region would still
//! have to earn.

use aex_identity_domain::credential::{Pepper, PresentedDigest, verifier};
use aex_regional_http::authz::{ParameterStore, SecretStore, TrustError};
use aex_regional_http::credential::{PresentedCredential, StoredVerifier};
use aex_regional_http::cursor::{CursorBinding, Order, SnapshotToken, SortTuple, decode, encode};
use aex_test_harness::MotoContainer;
use aex_wire::ids::{PrefixedId, Uuid7, WorkspaceId};
use aex_wire::routes::RouteId;
use aex_wire::types::{Region, Timestamp};
use aws_sdk_ssm::config::{BehaviorVersion, Credentials, Region as SigningRegion};
use aws_sdk_ssm::types::ParameterType;
use base64::Engine as _;

/// The id the credential pepper ring is held at, shaped as the deployed one: no
/// leading slash, and the *central* plane's id, because there is only one.
const PEPPER_SECRET: &str = "aex/integration/central/token-pepper";

/// The name the cursor signing ring is held at. The leading slash is what makes
/// it a hierarchy name rather than a flat one.
const CURSOR_PARAM: &str = "/aex/integration/regional/cursor-signing-key";

/// The pepper the fixture verifier is computed under.
const PEPPER: [u8; 32] = [11; 32];

/// The version that pepper is published as.
const PEPPER_VERSION: u16 = 3;

fn ssm(engine: &MotoContainer) -> aws_sdk_ssm::Client {
    let config = aws_sdk_ssm::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(SigningRegion::new(engine.region()))
        .endpoint_url(engine.endpoint_url())
        .credentials_provider(credentials(engine))
        .build();
    aws_sdk_ssm::Client::from_conf(config)
}

fn secrets(engine: &MotoContainer) -> aws_sdk_secretsmanager::Client {
    let config = aws_sdk_secretsmanager::Config::builder()
        .behavior_version(aws_sdk_secretsmanager::config::BehaviorVersion::latest())
        .region(aws_sdk_secretsmanager::config::Region::new(engine.region()))
        .endpoint_url(engine.endpoint_url())
        .credentials_provider(credentials(engine))
        .build();
    aws_sdk_secretsmanager::Client::from_conf(config)
}

fn credentials(engine: &MotoContainer) -> Credentials {
    Credentials::new(
        engine.access_key_id(),
        engine.secret_access_key(),
        None,
        None,
        "aex-integration",
    )
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Stores one parameter the way the key admin does, replacing whatever the name
/// already held.
async fn put(client: &aws_sdk_ssm::Client, name: &str, kind: ParameterType, value: &str) {
    client
        .put_parameter()
        .name(name)
        .r#type(kind)
        .value(value)
        .overwrite(true)
        .send()
        .await
        .expect("the parameter is stored");
}

/// Creates one secret the way the rotation ceremony does.
async fn create(client: &aws_sdk_secretsmanager::Client, id: &str, value: &str) {
    client
        .create_secret()
        .name(id)
        .secret_string(value)
        .send()
        .await
        .expect("the secret is stored");
}

/// The pepper document, one entry per `(version, material)` pair.
fn pepper_document(entries: &[(u16, [u8; 32])]) -> String {
    let peppers: Vec<String> = entries
        .iter()
        .map(|(version, material)| {
            format!(r#"{{"version":{version},"material":"{}"}}"#, b64(material))
        })
        .collect();
    format!(r#"{{"schemaVersion":1,"peppers":[{}]}}"#, peppers.join(","))
}

/// A ring mid-rotation: one signing key and one that only verifies.
fn cursor_document(current: &str, overlap: &str) -> String {
    format!(
        r#"{{"schemaVersion":1,"current":{{"keyId":"{current}","material":"{}"}},"overlap":[{{"keyId":"{overlap}","material":"{}"}}]}}"#,
        b64(&[9; 32]),
        b64(&[4; 48])
    )
}

/// A syntactically complete workspace API key.
fn workspace_key() -> String {
    let workspace = WorkspaceId::from_uuid7(Uuid7::compose(1_754_051_696_789, [2; 10]));
    let key = aex_wire::ids::ApiKeyId::from_uuid7(Uuid7::compose(1_754_051_696_789, [7; 10]));
    format!(
        "aex_wk_{}_{}_{}_{}",
        Region::EuWest1.code(),
        suffix(workspace),
        suffix(key),
        b64(&[7; 32])
    )
}

fn suffix<I: PrefixedId>(id: I) -> String {
    String::from_utf8(id.uuid7().encode_suffix().to_vec()).expect("Crockford is ASCII")
}

fn binding() -> CursorBinding {
    CursorBinding {
        route: RouteId::SecretsList,
        principal_scope: [1; 32],
        region: Region::EuWest1,
        workspace_id: WorkspaceId::from_uuid7(Uuid7::compose(1_754_051_696_789, [2; 10])),
        session_id: None,
        query_hash: [0; 32],
        order: Order::Ascending,
        snapshot: SnapshotToken::new("secrets").expect("a snapshot token"),
    }
}

fn moment(millis: i64) -> Timestamp {
    Timestamp::from_unix_millis(millis).expect("a representable instant")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pepper_ring_read_out_of_secrets_manager_admits_the_credential_it_fingerprinted() {
    let engine = MotoContainer::start().await.expect("moto starts");
    let client = secrets(&engine);
    let store = SecretStore::new(client.clone());

    // A rotation in flight, which is the only state worth reading through a
    // service: the version existing verifiers were computed under, beside the
    // one new credentials will be.
    create(
        &client,
        PEPPER_SECRET,
        &pepper_document(&[(PEPPER_VERSION, PEPPER), (PEPPER_VERSION + 1, [12; 32])]),
    )
    .await;

    let ring = store
        .pepper_ring(PEPPER_SECRET)
        .await
        .expect("the stored document decodes");
    assert_eq!(
        ring.versions(),
        vec![PEPPER_VERSION, PEPPER_VERSION + 1],
        "both entries survived the round trip"
    );

    // The material the service served is the material the ring verifies under:
    // a verifier computed outside this process, exactly as the control plane
    // computes it, is admitted by the ring the service handed back.
    let token = workspace_key();
    let keyed = verifier(&Pepper::new(PEPPER), &PresentedDigest::of(&token));
    let credential =
        PresentedCredential::new(token.into_bytes()).expect("a workspace key is a credential");
    ring.admits(
        &credential,
        &StoredVerifier::new(*keyed.as_bytes(), PEPPER_VERSION),
    )
    .expect("the round-tripped pepper reproduces the stored verifier");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_secret_this_region_does_not_hold_stops_the_edge_rather_than_emptying_the_ring() {
    let engine = MotoContainer::start().await.expect("moto starts");
    let store = SecretStore::new(secrets(&engine));

    match store
        .pepper_ring(PEPPER_SECRET)
        .await
        .expect_err("nothing was ever stored under that id")
    {
        TrustError::Unreadable { name, reason } => {
            assert_eq!(name, PEPPER_SECRET);
            assert!(
                !reason.is_empty(),
                "the service's own report is carried, not swallowed"
            );
        }
        other => panic!("an absent secret must never become an empty pepper ring: {other}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cursor_ring_held_as_a_securestring_comes_back_able_to_sign_and_verify() {
    let engine = MotoContainer::start().await.expect("moto starts");
    let client = ssm(&engine);
    let store = ParameterStore::new(client.clone());
    put(
        &client,
        CURSOR_PARAM,
        ParameterType::SecureString,
        &cursor_document("2026-08", "2026-07"),
    )
    .await;

    let ring = store
        .cursor_key_ring(CURSOR_PARAM)
        .await
        .expect("the ring decodes");
    let binding = binding();
    let tuple = SortTuple::new(vec!["alpha".to_owned()]).expect("a tuple");
    let cursor =
        encode(ring.current(), &binding, &tuple, moment(1_000)).expect("the ring mints a cursor");
    assert_eq!(
        decode(&ring, &cursor, &binding, moment(2_000)).expect("and verifies its own"),
        tuple,
        "the material the service served is the material the ring signs under"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_parameter_this_region_does_not_hold_stops_the_edge_rather_than_emptying_the_ring() {
    let engine = MotoContainer::start().await.expect("moto starts");
    let store = ParameterStore::new(ssm(&engine));

    match store
        .cursor_key_ring(CURSOR_PARAM)
        .await
        .expect_err("nothing was ever put at that name")
    {
        TrustError::Unreadable { name, reason } => {
            assert_eq!(name, CURSOR_PARAM);
            assert!(
                !reason.is_empty(),
                "the service's own report is carried, not swallowed"
            );
        }
        other => panic!("an absent parameter must never become an empty cursor ring: {other}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_parameter_holding_only_blank_space_is_no_value_rather_than_a_document() {
    let engine = MotoContainer::start().await.expect("moto starts");
    let client = ssm(&engine);
    let store = ParameterStore::new(client.clone());

    // The service refuses an empty value outright, so the nearest an operator
    // can come to blanking key material is a value with nothing in it. That arm
    // of `read` is unreachable without a service to store it.
    put(&client, CURSOR_PARAM, ParameterType::String, " ").await;
    assert_eq!(
        store
            .read(CURSOR_PARAM)
            .await
            .expect_err("a blank value is not a document"),
        TrustError::Empty {
            name: CURSOR_PARAM.to_owned()
        }
    );

    // `read` asks for decryption unconditionally, so a `String` parameter is the
    // case where that flag must change nothing at all.
    let document = cursor_document("2026-08", "2026-07");
    put(&client, CURSOR_PARAM, ParameterType::String, &document).await;
    assert_eq!(
        store
            .read(CURSOR_PARAM)
            .await
            .expect("the value is served")
            .as_str(),
        document.as_str(),
        "a hierarchy name and a JSON body both survive unchanged"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_secret_holding_only_blank_space_is_no_value_rather_than_a_document() {
    let engine = MotoContainer::start().await.expect("moto starts");
    let client = secrets(&engine);
    let store = SecretStore::new(client.clone());

    create(&client, PEPPER_SECRET, " ").await;
    assert_eq!(
        store
            .read(PEPPER_SECRET)
            .await
            .expect_err("a blank value is not a document"),
        TrustError::Empty {
            name: PEPPER_SECRET.to_owned()
        }
    );
}
