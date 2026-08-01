//! Public managed-web policy and serialization conformance.

use aex_brain_managed_web::egress::{EgressPolicy, EgressRejection, validate};
use aex_brain_managed_web::serializer::html_to_markdown;
use url::Url;

#[test]
fn managed_web_contract_is_https_443_and_deterministic_markdown() {
    let policy = EgressPolicy::managed_web();
    let admitted = validate(&policy, "https://example.com/path").expect("public HTTPS URL");
    assert_eq!(admitted.host.as_ref(), "example.com");
    assert!(matches!(
        validate(&policy, "https://example.com:8443/path"),
        Err(EgressRejection::PortNotAllowed { port: 8443 })
    ));

    let base = Url::parse("https://example.com/docs/").expect("base URL");
    assert_eq!(
        html_to_markdown("<h1>Title</h1><p><a href=\"guide\">Read</a></p>", &base),
        "# Title\n\n[Read](https://example.com/docs/guide)"
    );
}
