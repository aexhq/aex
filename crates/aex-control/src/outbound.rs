//! Guarded outbound HTTP for Aex-managed capabilities.
//!
//! User-controlled URLs are checked before each request, including redirects. DNS resolution is
//! performed inside reqwest through a resolver that rejects every internal address, closing the
//! resolve-then-connect gap used by DNS-rebinding attacks.

use std::net::IpAddr;
use std::time::Duration;

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
            .map_err(|error| Error::Invalid(format!("URL {url:?}: {error}")))?;
        if parsed.scheme() != "https" {
            return Err(Error::Invalid(format!(
                "URL {url:?}: scheme {:?} is not allowed (HTTPS required)",
                parsed.scheme()
            )));
        }
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(Error::Invalid(format!(
                "URL {url:?}: userinfo is not allowed"
            )));
        }
        let host = parsed
            .host_str()
            .ok_or_else(|| Error::Invalid(format!("URL {url:?}: no host")))?;
        // Literal IPs bypass DNS resolution, so they need the same policy here.
        let bare = host.trim_start_matches('[').trim_end_matches(']');
        if let Ok(ip) = bare.parse::<IpAddr>()
            && let Some(reason) = deny_reason(&ip)
        {
            return Err(Error::Invalid(format!(
                "URL {url:?}: address is {reason} (SSRF guard)"
            )));
        }
        Ok(parsed)
    }
}

/// Why an address cannot be a public outbound target.
pub fn deny_reason(ip: &IpAddr) -> Option<&'static str> {
    match ip {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            if v4.is_loopback() {
                Some("loopback")
            } else if v4.is_private() {
                Some("private (RFC1918)")
            } else if v4.is_link_local() {
                Some("link-local (metadata service range)")
            } else if octets[0] == 100 && (octets[1] & 0xc0) == 64 {
                Some("carrier-grade NAT (RFC6598)")
            } else if v4.is_unspecified() {
                Some("unspecified")
            } else if v4.is_broadcast() || v4.is_multicast() {
                Some("broadcast/multicast")
            } else if octets[0] == 192 && octets[1] == 0 && octets[2] == 0 {
                Some("IETF protocol assignments (RFC6890)")
            } else {
                None
            }
        }
        IpAddr::V6(v6) => {
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return deny_reason(&IpAddr::V4(mapped));
            }
            let segments = v6.segments();
            if v6.is_loopback() {
                Some("loopback")
            } else if v6.is_unspecified() {
                Some("unspecified")
            } else if (segments[0] & 0xffc0) == 0xfe80 {
                Some("link-local")
            } else if (segments[0] & 0xfe00) == 0xfc00 {
                Some("unique-local (fc00::/7)")
            } else if v6.is_multicast() {
                Some("multicast")
            } else {
                None
            }
        }
    }
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
            if addresses.is_empty() {
                return Err(format!("DNS {host}: no addresses").into());
            }
            for address in &addresses {
                if let Some(reason) = deny_reason(&address.ip()) {
                    return Err(format!(
                        "DNS {host}: resolves to {} which is {reason} (SSRF guard)",
                        address.ip()
                    )
                    .into());
                }
            }
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
        for address in [
            "127.0.0.1",
            "10.0.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "255.255.255.255",
            "224.0.0.1",
            "192.0.0.170",
            "::1",
            "::",
            "fe80::1",
            "fc00::1",
            "ff02::1",
            "::ffff:10.0.0.1",
            "::ffff:169.254.169.254",
        ] {
            assert!(
                deny_reason(&ip(address)).is_some(),
                "{address} must be denied"
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
}
