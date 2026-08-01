use std::io::Write as _;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use crate::egress::{DnsResolver, EgressPolicy, EgressRejection, resolve_and_screen, validate};
use crate::fetch::{
    FetchFormat, FetchRejection, FetchRequest, decode_body, fetch, format_document,
    media_type_allowed,
};
use crate::search::{WebSearchCredential, WebSearchProviderId, normalize_brave, normalize_serper};
use crate::serializer::{html_to_markdown, html_to_text};

proptest::proptest! {
    #[test]
    fn credential_key_never_appears_in_debug_or_errors(suffix in "[A-Za-z0-9]{1,128}") {
        let secret = format!("credential-secret-{suffix}");
        let payload = serde_json::to_vec(&serde_json::json!({
            "provider": "serper",
            "apiKey": secret,
        })).expect("credential fixture");
        let credential = WebSearchCredential::parse(&payload).expect("valid credential");
        let debug = format!("{credential:?}");
        let error = FetchRejection::Transport.to_string();
        proptest::prop_assert!(!debug.contains(&secret));
        proptest::prop_assert!(!error.contains(&secret));
    }
}

#[test]
fn url_grammar_is_https_443_without_credentials_or_literals() {
    let policy = EgressPolicy::managed_web();
    for invalid in [
        "http://example.com/",
        "https://user:pass@example.com/",
        "https://example.com:80/",
        "https://example.com:8080/",
        "https://example.com:22/",
        "https://example.com:6379/",
        "file:///etc/passwd",
        "gopher://example.com/",
        "//evil.example/",
        "https://127.0.0.1/",
        "https://0.0.0.0/",
        "https://[::1]/",
        "https://[::ffff:169.254.169.254]/",
        "https://[64:ff9b::a9fe:a9fe]/",
        "https://[2002:a9fe:a9fe::]/",
        "https://metadata.google.internal/",
        "https://169.254.170.2/",
        "https://100.100.100.200/",
        "https://2130706433/",
        "https://0177.0.0.1/",
    ] {
        assert!(validate(&policy, invalid).is_err(), "accepted `{invalid}`");
    }
    assert!(matches!(
        validate(
            &policy,
            &format!("https://example.com/{}", "x".repeat(9_000))
        ),
        Err(EgressRejection::UrlTooLong { .. })
    ));
    assert!(validate(&policy, "https://example.com/path?q=1").is_ok());
}

#[tokio::test]
async fn every_denied_range_and_mixed_resolution_is_rejected() {
    let policy = EgressPolicy::managed_web();
    let denied = [
        "0.0.0.1",
        "10.0.0.1",
        "100.64.0.1",
        "127.0.0.1",
        "169.254.169.254",
        "172.16.0.1",
        "192.0.0.1",
        "192.0.2.1",
        "192.88.99.1",
        "192.168.0.1",
        "198.18.0.1",
        "198.51.100.1",
        "203.0.113.1",
        "224.0.0.1",
        "240.0.0.1",
        "255.255.255.255",
        "::",
        "::1",
        "100::1",
        "2001::1",
        "2001:2::1",
        "2001:db8::1",
        "fc00::1",
        "fe80::1",
        "ff00::1",
    ];
    for address in denied {
        let resolver = FakeResolver::new(vec![address.parse().expect("fixture IP")]);
        let parsed = validate(&policy, "https://host.example/").expect("URL grammar");
        assert!(
            resolve_and_screen(parsed, &resolver).await.is_err(),
            "accepted `{address}`"
        );
        assert_eq!(resolver.calls(), 1);
    }

    let mixed = FakeResolver::new(vec![
        IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34)),
        IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
    ]);
    let parsed = validate(&policy, "https://host.example/").expect("URL grammar");
    assert!(matches!(
        resolve_and_screen(parsed, &mixed).await,
        Err(EgressRejection::MixedPublicPrivate)
    ));
}

#[tokio::test]
async fn resolution_is_nonempty_bounded_and_preserved_for_pinned_connect() {
    let policy = EgressPolicy::managed_web();
    let parsed = validate(&policy, "https://host.example/").expect("URL grammar");
    assert!(matches!(
        resolve_and_screen(parsed.clone(), &FakeResolver::new(vec![])).await,
        Err(EgressRejection::EmptyResolution)
    ));
    let seventeen = (1..=17)
        .map(|last| IpAddr::V4(Ipv4Addr::new(93, 184, 216, last)))
        .collect();
    assert!(matches!(
        resolve_and_screen(parsed.clone(), &FakeResolver::new(seventeen)).await,
        Err(EgressRejection::TooManyAddresses { n: 17 })
    ));

    let public = vec![
        IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34)),
        IpAddr::V6(
            "2606:2800:220:1:248:1893:25c8:1946"
                .parse::<Ipv6Addr>()
                .expect("fixture IP"),
        ),
    ];
    let resolver = FakeResolver::new(public.clone());
    let target = resolve_and_screen(parsed, &resolver)
        .await
        .expect("public target");
    assert_eq!(target.addrs, public);
    assert_eq!(resolver.calls(), 1, "screening performs one lookup");
}

struct FakeResolver {
    addresses: Vec<IpAddr>,
    calls: std::sync::atomic::AtomicUsize,
}

impl FakeResolver {
    fn new(addresses: Vec<IpAddr>) -> Self {
        Self {
            addresses,
            calls: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl DnsResolver for FakeResolver {
    async fn resolve(&self, _host: &str) -> Result<Vec<IpAddr>, EgressRejection> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(self.addresses.clone())
    }
}

#[test]
fn html_serialization_is_deterministic_and_drops_active_content() {
    let base = url::Url::parse("https://example.com/docs/page").expect("fixture URL");
    let html = r#"<!doctype html><html><head><title>discarded</title></head><body>
      <h1>Hello &amp; goodbye</h1><script>steal()</script>
      <p>Read <a href="../guide">the guide</a>.</p>
      <ul><li>first</li><li><code>second()</code></li></ul>
      <pre>  exact\ntext</pre><table><tr><th>A</th><th>B</th></tr><tr><td>1</td><td>2</td></tr></table>
    </body></html>"#;
    let expected = "# Hello & goodbye\n\nRead [the guide](https://example.com/guide).\n\n- first\n- `second()`\n\n```\n  exact\\ntext\n```\n\n| A | B |\n| 1 | 2 |";
    for _ in 0..100 {
        assert_eq!(html_to_markdown(html, &base), expected);
    }
    assert_eq!(
        html_to_text(html),
        "Hello & goodbye Read the guide. first second() exact\\ntext A B 1 2"
    );
}

#[test]
fn media_and_formatting_are_closed_and_bounded() {
    for media in [
        "text/html",
        "application/xhtml+xml",
        "text/plain",
        "text/markdown",
        "text/csv",
        "text/xml",
        "application/xml",
        "application/json",
        "application/ld+json",
    ] {
        assert!(media_type_allowed(media), "refused `{media}`");
    }
    for media in [
        "application/pdf",
        "image/png",
        "application/octet-stream",
        "*/*",
    ] {
        assert!(!media_type_allowed(media), "accepted `{media}`");
    }

    let base = url::Url::parse("https://example.com/").expect("fixture URL");
    assert_eq!(
        format_document(
            b"<p>Hello <b>world</b></p>",
            "text/html",
            FetchFormat::Markdown,
            &base
        )
        .expect("HTML is allowed"),
        "Hello world"
    );
    assert!(matches!(
        format_document(b"%PDF", "application/pdf", FetchFormat::Raw, &base),
        Err(FetchRejection::MediaTypeNotAllowed { .. })
    ));
}

#[tokio::test]
async fn fetch_rejects_invalid_bounds_before_resolution() {
    for max_bytes in [0, 1_023, 500_001, usize::MAX] {
        let resolver = FakeResolver::new(vec![IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))]);
        assert!(matches!(
            fetch(
                FetchRequest {
                    url: "https://example.com/",
                    format: FetchFormat::Markdown,
                    max_bytes,
                },
                &resolver,
            )
            .await,
            Err(FetchRejection::InvalidMaxBytes { value }) if value == max_bytes
        ));
        assert_eq!(resolver.calls(), 0);
    }
}

#[test]
fn response_decompression_is_closed_bounded_and_ratio_checked() {
    let body = b"bounded response";
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gzip.write_all(body).expect("gzip fixture");
    let gzip = gzip.finish().expect("gzip fixture");
    assert_eq!(decode_body(&gzip, Some("gzip"), 1_024).expect("gzip"), body);

    let mut encoded = Vec::new();
    {
        let mut writer = brotli::CompressorWriter::new(&mut encoded, 4_096, 5, 22);
        writer.write_all(body).expect("Brotli fixture");
    }
    assert_eq!(
        decode_body(&encoded, Some("br"), 1_024).expect("Brotli"),
        body
    );
    assert_eq!(
        decode_body(body, Some("deflate"), 1_024),
        Err(FetchRejection::ContentEncodingNotAllowed)
    );

    let bomb = vec![b'x'; 200_000];
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    gzip.write_all(&bomb).expect("bomb fixture");
    let gzip = gzip.finish().expect("bomb fixture");
    assert_eq!(
        decode_body(&gzip, Some("gzip"), 500_000),
        Err(FetchRejection::CompressionRatio)
    );
}

#[test]
fn search_credentials_are_closed_and_redacted() {
    let credential =
        WebSearchCredential::parse(br#"{"provider":"brave","apiKey":"not-for-output"}"#)
            .expect("valid credential");
    assert_eq!(credential.provider(), WebSearchProviderId::Brave);
    assert!(!format!("{credential:?}").contains("not-for-output"));
    for malformed in [
        br#"{"provider":"other","apiKey":"x"}"#.as_slice(),
        br#"{"provider":"brave","apiKey":""}"#.as_slice(),
        br#"{"provider":"brave","apiKey":"x","endpoint":"https://evil.invalid"}"#.as_slice(),
    ] {
        assert!(WebSearchCredential::parse(malformed).is_err());
    }
}

#[test]
fn provider_fixtures_normalize_in_returned_order_and_drop_invalid_items() {
    let brave = serde_json::json!({"web":{"results":[
        {"title":"A", "url":"https://a.example/", "description":"one", "extra":true},
        {"url":"https://missing-title.example/"},
        {"title":"B", "url":"https://b.example/", "description":"two"},
        {"title":"C", "url":"https://c.example/"}
    ]}});
    let brave = normalize_brave(&brave, "query", 2).expect("Brave fixture");
    assert_eq!(brave.provider, WebSearchProviderId::Brave);
    assert_eq!(brave.results.len(), 2);
    assert_eq!(brave.results[0].rank, 1);
    assert_eq!(brave.results[1].rank, 2);
    assert!(brave.truncated, "invalid and excess items are counted");

    let serper = serde_json::json!({"organic":[
        {"title":"First", "link":"https://first.example/", "snippet":"one"},
        {"title":"missing link"},
        {"title":"Second", "link":"https://second.example/", "date":"2026-08-01T00:00:00Z"}
    ], "unknown":"ignored"});
    let serper = normalize_serper(&serper, "query", 20).expect("Serper fixture");
    assert_eq!(serper.provider, WebSearchProviderId::Serper);
    assert_eq!(serper.results.len(), 2);
    assert!(serper.truncated);

    let empty = normalize_serper(&serde_json::json!({}), "query", 20).expect("empty fixture");
    assert!(empty.results.is_empty());
    assert!(!empty.truncated);
}
