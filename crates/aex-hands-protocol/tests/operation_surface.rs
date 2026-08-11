//! Operation arms and capability boundaries the guest must enforce.
//!
//! `Materialize` needs a grant because the credential-free guest cannot resolve
//! a bare `ContentHash`, `ProcessStatus` returns a bounded window, and the
//! browser capability gate is explicit. Each rule below is one a guest executor
//! is allowed to depend on.

use aex_hands_protocol::operation::{
    BrowserCommand, BrowserViewport, ContentEndpoint, ContentEndpointError, GuestPath,
    GuestProcessId, GuestRoot, OperationRequest, PlanRejection, PresignedPlan, TransferDirection,
};
use aex_wire::ids::ContentHash;
use aex_wire::types::{HttpsUrl, Timestamp};

fn moment(millis: i64) -> Timestamp {
    Timestamp::from_unix_millis(millis).expect("timestamp")
}

fn digest(byte: u8) -> ContentHash {
    ContentHash::from_bytes([byte; 32])
}

fn endpoint() -> ContentEndpoint {
    ContentEndpoint::parse("https://euw1.content.aex.dev").expect("endpoint")
}

fn url(text: &str) -> HttpsUrl {
    HttpsUrl::parse(text).expect("url")
}

fn fetch_plan() -> PresignedPlan {
    PresignedPlan::fetch(
        url("https://euw1.content.aex.dev/plans/abc?X-Amz-Signature=deadbeef"),
        moment(1_785_501_596_000),
        digest(0x11),
        1_048_576,
    )
}

fn store_plan() -> PresignedPlan {
    PresignedPlan::store(
        url("https://euw1.content.aex.dev/manifests/abc?X-Amz-Signature=deadbeef"),
        moment(1_785_501_596_000),
        1_048_576,
    )
}

// ---------------------------------------------------------------------------
// The presigned plan
// ---------------------------------------------------------------------------

#[test]
fn a_content_endpoint_is_a_bare_https_origin() {
    assert_eq!(
        ContentEndpoint::parse("https://euw1.content.aex.dev")
            .expect("origin")
            .as_str(),
        "https://euw1.content.aex.dev"
    );
    assert!(ContentEndpoint::parse("https://euw1.content.aex.dev:8443").is_ok());
    // Case is normalized so a comparison cannot be defeated by spelling.
    assert_eq!(
        ContentEndpoint::parse("https://EUW1.Content.AEX.dev").expect("origin"),
        endpoint()
    );

    assert_eq!(
        ContentEndpoint::parse("http://euw1.content.aex.dev"),
        Err(ContentEndpointError::NotAnHttpsOrigin)
    );
    for rejected in [
        "https://euw1.content.aex.dev/plans",
        "https://euw1.content.aex.dev?a=1",
        "https://euw1.content.aex.dev#f",
        "https://user@euw1.content.aex.dev",
    ] {
        assert_eq!(
            ContentEndpoint::parse(rejected),
            Err(ContentEndpointError::NotBareOrigin),
            "`{rejected}` must not be a content endpoint"
        );
    }
    assert_eq!(
        ContentEndpoint::parse("https://"),
        Err(ContentEndpointError::Host)
    );
}

#[test]
fn a_plan_url_is_unreachable_without_the_pinned_endpoint() {
    let plan = fetch_plan();
    let authorized = plan
        .authorize(
            &endpoint(),
            moment(1_785_501_296_000),
            TransferDirection::Fetch,
        )
        .expect("a live plan for the pinned origin authorizes");
    assert!(
        authorized
            .as_str()
            .starts_with("https://euw1.content.aex.dev/")
    );

    // The guest is launched against exactly one origin. A plan naming any other
    // host is refused, so a forged frame cannot point the guest at an attacker.
    let foreign = ContentEndpoint::parse("https://euw2.content.aex.dev").expect("endpoint");
    assert!(matches!(
        plan.authorize(
            &foreign,
            moment(1_785_501_296_000),
            TransferDirection::Fetch
        ),
        Err(PlanRejection::ForeignOrigin { .. })
    ));
}

#[test]
fn a_plan_stops_working_at_its_expiry_and_in_the_wrong_direction() {
    let plan = fetch_plan();
    assert!(
        plan.authorize(
            &endpoint(),
            moment(1_785_501_596_000),
            TransferDirection::Fetch
        )
        .is_ok(),
        "the expiry instant itself is still inside the window"
    );
    assert!(matches!(
        plan.authorize(
            &endpoint(),
            moment(1_785_501_596_001),
            TransferDirection::Fetch
        ),
        Err(PlanRejection::Expired { .. })
    ));
    assert!(matches!(
        plan.authorize(
            &endpoint(),
            moment(1_785_501_296_000),
            TransferDirection::Store
        ),
        Err(PlanRejection::WrongDirection { .. })
    ));
}

#[test]
fn a_fetch_plan_must_pin_a_digest_and_a_store_plan_must_not() {
    let now = moment(1_785_501_296_000);
    let mut incoherent = fetch_plan();
    incoherent.expected_digest = None;
    assert_eq!(
        incoherent.authorize(&endpoint(), now, TransferDirection::Fetch),
        Err(PlanRejection::MissingDigest)
    );

    let mut incoherent = store_plan();
    incoherent.expected_digest = Some(digest(0x22));
    assert_eq!(
        incoherent.authorize(&endpoint(), now, TransferDirection::Store),
        Err(PlanRejection::UnexpectedDigest)
    );
}

#[test]
fn a_plan_declares_a_byte_ceiling_inside_the_published_bound() {
    let now = moment(1_785_501_296_000);
    let zero = PresignedPlan::store(
        url("https://euw1.content.aex.dev/manifests/abc"),
        moment(1_785_501_596_000),
        0,
    );
    assert!(matches!(
        zero.authorize(&endpoint(), now, TransferDirection::Store),
        Err(PlanRejection::Unbounded { .. })
    ));
    let huge = PresignedPlan::store(
        url("https://euw1.content.aex.dev/manifests/abc"),
        moment(1_785_501_596_000),
        PresignedPlan::MAX_PLAN_BYTES + 1,
    );
    assert!(matches!(
        huge.authorize(&endpoint(), now, TransferDirection::Store),
        Err(PlanRejection::Unbounded { .. })
    ));
}

#[test]
fn a_plan_never_renders_its_signature() {
    // A presigned URL is bearer material for one object. Anything that logs a
    // request must not thereby log the capability.
    let rendered = format!("{:?}", fetch_plan());
    assert!(
        !rendered.contains("deadbeef"),
        "a plan leaked its signature: {rendered}"
    );
    assert!(
        rendered.contains("euw1.content.aex.dev"),
        "a plan must still say which origin it names: {rendered}"
    );
    let request = OperationRequest::Materialize {
        root: digest(0x33),
        plan: fetch_plan(),
    };
    assert!(!format!("{request:?}").contains("deadbeef"));
    // Serialization is the wire, and the guest needs the URL, so it is present.
    let encoded = serde_json::to_string(&fetch_plan()).expect("serialize");
    assert!(encoded.contains("deadbeef"));
    let round_trip: PresignedPlan = serde_json::from_str(&encoded).expect("decode");
    assert_eq!(round_trip, fetch_plan());
}

#[test]
fn materialize_carries_a_grant_rather_than_a_bare_hash() {
    // H-BOUNDARY: the guest holds no AWS credential, so a `ContentHash` alone is
    // not a thing it can act on. `root` stays as identity for the call hash; the
    // grant is what moves bytes.
    let materialize = OperationRequest::Materialize {
        root: digest(0x44),
        plan: fetch_plan(),
    };
    let OperationRequest::Materialize { plan, .. } = &materialize else {
        panic!("materialize");
    };
    assert_eq!(plan.direction, TransferDirection::Fetch);
}

// ---------------------------------------------------------------------------
// Windowed process output
// ---------------------------------------------------------------------------

#[test]
fn process_status_reads_a_bounded_window_rather_than_a_tail() {
    let request = OperationRequest::ProcessStatus {
        process: GuestProcessId("op_01kyw2qa4ne00r40r40m30e209".to_owned()),
        from_offset: 4_096,
        max_bytes: 65_536,
    };
    let OperationRequest::ProcessStatus {
        from_offset,
        max_bytes,
        ..
    } = &request
    else {
        panic!("process status");
    };
    assert_eq!(*from_offset, 4_096);
    assert_eq!(*max_bytes, 65_536);
    assert_eq!(OperationRequest::MAX_OUTPUT_WINDOW_BYTES, 1_000_000);

    let encoded = serde_json::to_string(&request).expect("serialize");
    assert!(encoded.contains("\"fromOffset\":4096"));
    assert!(encoded.contains("\"maxBytes\":65536"));
    let decoded: OperationRequest = serde_json::from_str(&encoded).expect("decode");
    assert_eq!(decoded, request);

    // Strict decode: the pre-window spelling no longer parses, so a stale sender
    // is a typed failure rather than a silent tail read.
    assert!(
        serde_json::from_str::<OperationRequest>(
            r#"{"operation":"process_status","process":"op_01kyw2qa4ne00r40r40m30e209"}"#
        )
        .is_err()
    );
}

// ---------------------------------------------------------------------------
// The browser arm
// ---------------------------------------------------------------------------

#[test]
fn the_browser_capability_gate_has_something_to_fire_on() {
    let open = OperationRequest::Browser {
        session: None,
        command: BrowserCommand::Open {
            url: url("https://example.test/start"),
            viewport: Some(BrowserViewport {
                width: 1280,
                height: 720,
            }),
            timeout_ms: 30_000,
        },
    };
    assert!(open.requires_browser());
    assert!(
        !OperationRequest::StatPath {
            path: GuestPath::parse(&GuestRoot::workspace(), "/workspace").expect("path"),
        }
        .requires_browser()
    );
}

#[test]
fn only_open_mints_a_session_and_every_other_command_names_one() {
    let open = BrowserCommand::Open {
        url: url("https://example.test/start"),
        viewport: None,
        timeout_ms: 30_000,
    };
    assert!(!open.needs_session());
    let session = GuestProcessId("op_01kyw2qa4ne00r40r40m30e209".to_owned());
    for command in [
        BrowserCommand::Navigate {
            url: url("https://example.test/next"),
        },
        BrowserCommand::Click {
            selector: "#submit".to_owned(),
        },
        BrowserCommand::Type {
            selector: "#q".to_owned(),
            text: "hello".to_owned(),
        },
        BrowserCommand::Key {
            key: "Enter".to_owned(),
        },
        BrowserCommand::Scroll {
            selector: None,
            delta_y: -400,
        },
        BrowserCommand::Wait { ms: 250 },
        BrowserCommand::Screenshot,
        BrowserCommand::ReadText { selector: None },
        BrowserCommand::Evaluate {
            expression: "document.title".to_owned(),
        },
        BrowserCommand::Close,
    ] {
        assert!(command.needs_session(), "{command:?}");
        let request = OperationRequest::Browser {
            session: Some(session.clone()),
            command: command.clone(),
        };
        assert!(request.requires_browser());
        assert!(request.browser_target_is_coherent());
        let stray = OperationRequest::Browser {
            session: None,
            command,
        };
        assert!(
            !stray.browser_target_is_coherent(),
            "a session-bound command with no session must be refused"
        );
    }
    assert!(
        !OperationRequest::Browser {
            session: Some(session),
            command: open,
        }
        .browser_target_is_coherent(),
        "opening onto an existing session is not a thing"
    );
}

#[test]
fn every_browser_operand_is_bounded() {
    assert!(
        BrowserCommand::Click {
            selector: "#ok".to_owned()
        }
        .is_bounded()
    );
    assert!(
        !BrowserCommand::Click {
            selector: "x".repeat(BrowserCommand::MAX_SELECTOR_BYTES + 1)
        }
        .is_bounded()
    );
    assert!(
        !BrowserCommand::Type {
            selector: "#q".to_owned(),
            text: "x".repeat(BrowserCommand::MAX_TEXT_BYTES + 1),
        }
        .is_bounded()
    );
    assert!(!BrowserCommand::Wait { ms: 30_001 }.is_bounded());
    assert!(BrowserCommand::Wait { ms: 30_000 }.is_bounded());
    assert!(
        !BrowserCommand::Open {
            url: url("https://example.test/"),
            viewport: Some(BrowserViewport {
                width: 319,
                height: 720
            }),
            timeout_ms: 30_000,
        }
        .is_bounded()
    );
    assert!(
        !BrowserCommand::Open {
            url: url("https://example.test/"),
            viewport: None,
            timeout_ms: 120_001,
        }
        .is_bounded()
    );
    assert!(
        !BrowserCommand::Evaluate {
            expression: "x".repeat(BrowserCommand::MAX_EXPRESSION_BYTES + 1),
        }
        .is_bounded()
    );
}
