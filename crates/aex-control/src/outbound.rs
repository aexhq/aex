//! Guarded outbound HTTP for Aex-managed capabilities.
//!
//! User-controlled URLs are checked before each request, including redirects. DNS resolution is
//! performed inside reqwest through a resolver that rejects every internal address, closing the
//! resolve-then-connect gap used by DNS-rebinding attacks.

use std::net::IpAddr;
use std::time::Duration;

use brain_protocol::network::special_use_reason;

use crate::{Error, Result};

#[derive(Clone)]
pub struct Outbound {
    client: reqwest::Client,
}

impl std::fmt::Debug for Outbound {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Outbound").finish_non_exhaustive()
    }
}

impl Outbound {
    pub fn guarded() -> Self {
        let client = reqwest::Client::builder()
            // An inherited HTTP(S)_PROXY would move DNS resolution outside this process and could
            // bypass the resolver policy for user-controlled fetch targets.
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .pool_max_idle_per_host(4)
            .dns_resolver(GuardingResolver)
            .build()
            .expect("guarded outbound HTTP client");
        Self { client }
    }

    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// Check one user- or provider-selected target before giving it to the guarded client.
    pub fn check_url(&self, url: &str) -> Result<reqwest::Url> {
        let parsed = reqwest::Url::parse(url)
            .map_err(|error| Error::Invalid(format!("invalid outbound URL: {error}")))?;
        if parsed.scheme() != "https" {
            return Err(Error::Invalid(format!(
                "outbound URL scheme {:?} is not allowed (HTTPS required)",
                parsed.scheme()
            )));
        }
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(Error::Invalid(
                "outbound URL userinfo is not allowed".into(),
            ));
        }
        if parsed.fragment().is_some() {
            return Err(Error::Invalid(
                "outbound URL fragments are not allowed".into(),
            ));
        }
        let host = parsed
            .host_str()
            .ok_or_else(|| Error::Invalid("outbound URL has no host".into()))?;
        // Literal IPs bypass DNS resolution, so they need the same policy here.
        let bare = host.trim_start_matches('[').trim_end_matches(']');
        if let Ok(ip) = bare.parse::<IpAddr>()
            && let Some(reason) = special_use_reason(&ip)
        {
            return Err(Error::Invalid(format!(
                "outbound URL address is {reason} (SSRF guard)"
            )));
        }
        Ok(parsed)
    }
}

fn validate_resolved_addresses(
    host: &str,
    addresses: &[std::net::SocketAddr],
) -> std::result::Result<(), String> {
    if addresses.is_empty() {
        return Err(format!("DNS {host}: no addresses"));
    }
    for address in addresses {
        if let Some(reason) = special_use_reason(&address.ip()) {
            return Err(format!(
                "DNS {host}: resolves to {} which is {reason} (SSRF guard)",
                address.ip()
            ));
        }
    }
    Ok(())
}

#[derive(Debug)]
struct GuardingResolver;

impl reqwest::dns::Resolve for GuardingResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            // Reqwest replaces the placeholder port with the URL's actual port.
            let addresses: Vec<std::net::SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(|error| -> Box<dyn std::error::Error + Send + Sync> {
                    format!("DNS {host}: {error}").into()
                })?
                .collect();
            validate_resolved_addresses(&host, &addresses)
                .map_err(|error| -> Box<dyn std::error::Error + Send + Sync> { error.into() })?;
            Ok(Box::new(addresses.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(value: &str) -> IpAddr {
        value.parse().unwrap()
    }

    #[test]
    fn deny_table_refuses_internal_address_classes() {
        for &(address, expected) in brain_protocol::network::SPECIAL_USE_FIXTURES {
            assert!(
                special_use_reason(&ip(address)).is_some(),
                "{address} must be denied"
            );
            assert_eq!(special_use_reason(&ip(address)), Some(expected));
        }
        for &address in brain_protocol::network::PUBLIC_UNICAST_FIXTURES {
            assert_eq!(
                special_use_reason(&ip(address)),
                None,
                "{address} must stay public"
            );
        }
    }

    #[test]
    fn guarded_urls_require_public_https_without_userinfo() {
        let outbound = Outbound::guarded();
        for url in [
            "http://example.com/",
            "https://user:password@example.com/",
            "https://127.0.0.1/",
            "https://169.254.169.254/latest/meta-data/",
            "https://[::1]/",
            "https://example.com/path#fragment",
            "ftp://example.com/",
        ] {
            assert!(outbound.check_url(url).is_err(), "{url} must be refused");
        }
        assert!(outbound.check_url("https://example.com/path").is_ok());
    }

    #[tokio::test]
    async fn guarded_resolver_refuses_internal_dns_results() {
        let error = Outbound::guarded()
            .client()
            .get("https://localhost:1/nope")
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .expect_err("localhost must not connect");
        assert!(format!("{error:?}").contains("SSRF guard"));
    }

    #[test]
    fn one_denied_answer_rejects_a_mixed_dns_set() {
        let public = "93.184.216.34:443".parse().unwrap();
        let private = "10.0.0.7:443".parse().unwrap();
        let documentation = "[2001:db8::7]:443".parse().unwrap();
        assert!(validate_resolved_addresses("mixed.test", &[public, private]).is_err());
        assert!(validate_resolved_addresses("mixed.test", &[public, documentation]).is_err());
        assert!(validate_resolved_addresses("public.test", &[public]).is_ok());
    }
}
