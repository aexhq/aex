//! The cold-start Parameter Store reader, against `moto`'s SSM.
//!
//! What this proves: that [`ParameterStore`] round-trips the two documents a
//! regional edge cannot start without, through a service rather than a fake. A
//! hierarchy name goes where `GetParameter` expects it, the value an operator
//! put comes back byte for byte, the `WithDecryption` this reader always sets is
//! accepted on a `String` and on a `SecureString` alike, a ring read out of the
//! service really signs and verifies a cursor, and the two failure arms a
//! service owns — an absent name and a blank value — arrive as
//! `TrustError::Unreadable` and `TrustError::Empty` rather than as an empty
//! anchor set. The scripted `authz` target already fixes what these documents
//! *mean*; only a service fixes that the bytes arrive at all, which is the half
//! a composition root depends on before its listener binds.
//!
//! What it does not prove is stated rather than assumed. `moto` has no KMS
//! behind a `SecureString` — `release/policy/test-images.toml` lists exactly
//! that under `cannot_prove` — so the key the cursor ring is really sealed
//! under, its key policy and its grants stay live concerns, and a green run here
//! says nothing about a role that can read the parameter but not decrypt it.
//! `moto` evaluates no IAM either, so the path scoping that holds a regional
//! execution role to its own plane's prefix is untested: every read below
//! succeeds for reasons a deployed region would still have to earn. Parameter
//! policies and the tier limits that bound them are absent for the same reason.

use aex_identity_domain::assertion::KeyId;
use aex_regional_http::authz::{ParameterStore, TrustError};
use aex_regional_http::cursor::{CursorBinding, Order, SnapshotToken, SortTuple, decode, encode};
use aex_test_harness::MotoContainer;
use aex_wire::ids::{PrefixedId, Uuid7, WorkspaceId};
use aex_wire::routes::RouteId;
use aex_wire::types::{Region, Timestamp};
use aws_sdk_ssm::Client;
use aws_sdk_ssm::config::{BehaviorVersion, Credentials, Region as SigningRegion};
use aws_sdk_ssm::types::ParameterType;
use base64::Engine as _;
use uuid::Uuid;

/// The name the trust anchors are held at, shaped as a deployed one: the leading
/// slash is what makes it a hierarchy name rather than a flat one.
const ANCHOR_PARAM: &str = "/aex/integration/authz/verify-keys";

/// The name the cursor signing ring is held at.
const CURSOR_PARAM: &str = "/aex/integration/regional/cursor-signing-key";

/// The assertion signing identity the anchor document names.
const KID: Uuid = Uuid::from_u128(0x2026_080a);

/// The identity of a key that is on its way out of the ring.
const RETIRING_KID: Uuid = Uuid::from_u128(0x2026_0709);

/// The instant the anchor assertions are made at.
const NOW_MS: u64 = 1_754_051_698_000;

fn client(engine: &MotoContainer) -> Client {
    let config = aws_sdk_ssm::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(SigningRegion::new(engine.region()))
        .endpoint_url(engine.endpoint_url())
        .credentials_provider(Credentials::new(
            engine.access_key_id(),
            engine.secret_access_key(),
            None,
            None,
            "aex-integration",
        ))
        .build();
    Client::from_conf(config)
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Stores one parameter the way the key admin does, replacing whatever the name
/// already held.
async fn put(client: &Client, name: &str, kind: ParameterType, value: &str) {
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

/// The anchor document, one entry per `(identity, notAfterMs)` pair.
fn anchor_document(entries: &[(Uuid, u64)]) -> String {
    let keys: Vec<String> = entries
        .iter()
        .enumerate()
        .map(|(index, (key_id, not_after_ms))| {
            let seed = u8::try_from(index).expect("a handful of anchors");
            format!(
                r#"{{"keyId":"{key_id}","publicKey":"{}","notAfterMs":{not_after_ms}}}"#,
                b64(&[seed; 32])
            )
        })
        .collect();
    format!(r#"{{"schemaVersion":1,"keys":[{}]}}"#, keys.join(","))
}

/// A ring mid-rotation: one signing key and one that only verifies.
fn cursor_document(current: &str, overlap: &str) -> String {
    format!(
        r#"{{"schemaVersion":1,"current":{{"keyId":"{current}","material":"{}"}},"overlap":[{{"keyId":"{overlap}","material":"{}"}}]}}"#,
        b64(&[9; 32]),
        b64(&[4; 48])
    )
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
async fn the_anchor_set_a_region_starts_with_is_the_document_the_service_holds() {
    let engine = MotoContainer::start().await.expect("moto starts");
    let client = client(&engine);
    let store = ParameterStore::new(client.clone());

    // A rotation in flight, which is the only state worth reading through a
    // service: the incoming key, and the one this region stops accepting at
    // `notAfterMs` without a redeploy.
    let document = anchor_document(&[(KID, u64::MAX), (RETIRING_KID, NOW_MS)]);
    put(&client, ANCHOR_PARAM, ParameterType::String, &document).await;

    let anchors = store
        .trust_anchors(ANCHOR_PARAM)
        .await
        .expect("the stored document decodes");
    assert_eq!(anchors.len(), 2, "both entries survived the round trip");
    assert!(anchors.find(KeyId::new(KID), NOW_MS).is_some());
    assert!(anchors.find(KeyId::new(RETIRING_KID), NOW_MS - 1).is_some());
    assert!(
        anchors.find(KeyId::new(RETIRING_KID), NOW_MS).is_none(),
        "the lapse instant is the document's, not the service's"
    );

    // `read` asks for decryption unconditionally, so a `String` parameter is the
    // case where that flag must change nothing at all.
    assert_eq!(
        store
            .read(ANCHOR_PARAM)
            .await
            .expect("the value is served")
            .as_str(),
        document.as_str(),
        "a hierarchy name and a JSON body both survive unchanged"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cursor_ring_held_as_a_securestring_comes_back_able_to_sign_and_verify() {
    let engine = MotoContainer::start().await.expect("moto starts");
    let client = client(&engine);
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
async fn a_parameter_this_region_does_not_hold_stops_the_edge_rather_than_emptying_it() {
    let engine = MotoContainer::start().await.expect("moto starts");
    let store = ParameterStore::new(client(&engine));

    match store
        .trust_anchors(ANCHOR_PARAM)
        .await
        .expect_err("nothing was ever put at that name")
    {
        TrustError::Unreadable { name, reason } => {
            assert_eq!(name, ANCHOR_PARAM);
            assert!(
                !reason.is_empty(),
                "the service's own report is carried, not swallowed"
            );
        }
        other => panic!("an absent parameter must never become an empty anchor set: {other}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_parameter_holding_only_blank_space_is_no_value_rather_than_a_document() {
    let engine = MotoContainer::start().await.expect("moto starts");
    let client = client(&engine);
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
}
