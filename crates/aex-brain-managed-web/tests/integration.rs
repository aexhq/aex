//! Resolver-to-connect pinning integration evidence.

use std::net::{IpAddr, Ipv4Addr};

use aex_brain_managed_web::egress::{
    DnsResolver, EgressPolicy, EgressRejection, resolve_and_screen, validate,
};

struct FixedResolver(Vec<IpAddr>);

#[async_trait::async_trait]
impl DnsResolver for FixedResolver {
    async fn resolve(&self, _host: &str) -> Result<Vec<IpAddr>, EgressRejection> {
        Ok(self.0.clone())
    }
}

#[tokio::test]
async fn admitted_dns_addresses_are_the_exact_pinned_connect_set() {
    let expected = vec![
        IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34)),
        IpAddr::V4(Ipv4Addr::new(93, 184, 216, 35)),
    ];
    let parsed =
        validate(&EgressPolicy::managed_web(), "https://example.com/").expect("public HTTPS URL");
    let screened = resolve_and_screen(parsed, &FixedResolver(expected.clone()))
        .await
        .expect("all addresses are public");

    assert_eq!(screened.addrs, expected);
}
