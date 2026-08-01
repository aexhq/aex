//! Shared Brain-originated egress URL, DNS, and address policy.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs as _};

use url::{Host, Url};

/// Static outbound policy for managed web and MCP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EgressPolicy {
    /// Allowed URL schemes.
    pub schemes: &'static [&'static str],
    /// Allowed explicit or implicit ports.
    pub ports: &'static [u16],
    /// Redirect ceiling.
    pub max_redirects: u8,
    /// Resolution cardinality ceiling.
    pub max_addresses: usize,
}

impl EgressPolicy {
    /// HTTPS-only, port-443-only Brain-managed egress.
    #[must_use]
    pub const fn managed_web() -> Self {
        Self {
            schemes: &["https"],
            ports: &[443],
            max_redirects: 5,
            max_addresses: 16,
        }
    }
}

/// A syntactically admitted target that still needs one DNS screening pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedTarget {
    /// Normalized URL, including punycode host rendering.
    pub url: Url,
    /// Normalized host sent to the resolver and pinned client.
    pub host: Box<str>,
}

/// A target whose complete address set is public and pinned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedTarget {
    /// Normalized URL.
    pub url: Url,
    /// Normalized DNS host.
    pub host: Box<str>,
    /// The exact screened address set used for connect.
    pub addrs: Vec<IpAddr>,
}

/// DNS is injected so qualification, redirects, tests, and runtime all use one
/// screening contract.
#[async_trait::async_trait]
pub trait DnsResolver: Send + Sync {
    /// Returns every A and AAAA record for one normalized host.
    async fn resolve(&self, host: &str) -> Result<Vec<IpAddr>, EgressRejection>;
}

/// Host resolver used by the production adapter.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemDnsResolver;

#[async_trait::async_trait]
impl DnsResolver for SystemDnsResolver {
    async fn resolve(&self, host: &str) -> Result<Vec<IpAddr>, EgressRejection> {
        let host = host.to_owned();
        tokio::task::spawn_blocking(move || {
            (host.as_str(), 443)
                .to_socket_addrs()
                .map(|addresses| addresses.map(|address| address.ip()).collect())
                .map_err(|_| EgressRejection::DnsFailure)
        })
        .await
        .map_err(|_| EgressRejection::DnsFailure)?
    }
}

/// Parses and checks everything knowable before DNS.
///
/// # Errors
///
/// Rejects relative/oversized URLs, credentials, non-HTTPS schemes, every
/// non-443 port, named metadata hosts, localhost, and denied IP literals.
pub fn validate(policy: &EgressPolicy, raw: &str) -> Result<ParsedTarget, EgressRejection> {
    if raw.len() > 8_192 {
        return Err(EgressRejection::UrlTooLong { bytes: raw.len() });
    }
    let url = Url::parse(raw).map_err(|_| EgressRejection::NotAbsolute)?;
    if !policy.schemes.contains(&url.scheme()) {
        return Err(EgressRejection::SchemeNotAllowed {
            scheme: url.scheme().into(),
        });
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(EgressRejection::CredentialsInUrl);
    }
    let port = url
        .port_or_known_default()
        .ok_or(EgressRejection::PortNotAllowed { port: 0 })?;
    if !policy.ports.contains(&port) {
        return Err(EgressRejection::PortNotAllowed { port });
    }
    let host = match url.host().ok_or(EgressRejection::NotAbsolute)? {
        Host::Domain(domain) => {
            if domain.eq_ignore_ascii_case("localhost")
                || domain.eq_ignore_ascii_case("metadata.google.internal")
            {
                return Err(EgressRejection::PrivateHostName {
                    host: domain.to_owned(),
                });
            }
            domain.to_owned().into_boxed_str()
        }
        Host::Ipv4(address) => {
            if let Some(range) = denied(IpAddr::V4(address)) {
                return Err(EgressRejection::HostIsIpLiteralDenied {
                    addr: IpAddr::V4(address),
                    range,
                });
            }
            address.to_string().into_boxed_str()
        }
        Host::Ipv6(address) => {
            if let Some(range) = denied(IpAddr::V6(address)) {
                return Err(EgressRejection::HostIsIpLiteralDenied {
                    addr: IpAddr::V6(address),
                    range,
                });
            }
            address.to_string().into_boxed_str()
        }
    };
    Ok(ParsedTarget { url, host })
}

/// Resolves exactly once, screens the complete set, and returns those same
/// addresses for pinned connect.
///
/// # Errors
///
/// Rejects an empty or oversized resolution and the whole target if any record
/// is private, link-local, metadata, documentation, multicast, or reserved.
pub async fn resolve_and_screen(
    target: ParsedTarget,
    resolver: &dyn DnsResolver,
) -> Result<ValidatedTarget, EgressRejection> {
    let addresses = resolver.resolve(&target.host).await?;
    if addresses.is_empty() {
        return Err(EgressRejection::EmptyResolution);
    }
    if addresses.len() > 16 {
        return Err(EgressRejection::TooManyAddresses { n: addresses.len() });
    }
    let mut first_denied = None;
    let mut has_public = false;
    for address in &addresses {
        if let Some(range) = denied(*address) {
            first_denied.get_or_insert((*address, range));
        } else {
            has_public = true;
        }
    }
    if let Some((addr, range)) = first_denied {
        if has_public {
            return Err(EgressRejection::MixedPublicPrivate);
        }
        return Err(EgressRejection::PrivateAddress { addr, range });
    }
    Ok(ValidatedTarget {
        url: target.url,
        host: target.host,
        addrs: addresses,
    })
}

fn denied(address: IpAddr) -> Option<&'static str> {
    match address {
        IpAddr::V4(address) => denied_v4(address),
        IpAddr::V6(address) => {
            if let Some(mapped) = address.to_ipv4_mapped() {
                return denied_v4(mapped);
            }
            let value = u128::from(address);
            if in_v6(
                value,
                u128::from(Ipv6Addr::new(0x64, 0xff9b, 0, 0, 0, 0, 0, 0)),
                96,
            ) || in_v6(
                value,
                u128::from(Ipv6Addr::new(0x64, 0xff9b, 1, 0, 0, 0, 0, 0)),
                48,
            ) {
                let embedded = u32::try_from(value & u128::from(u32::MAX))
                    .expect("IPv4 mask is bounded to u32");
                return denied_v4(Ipv4Addr::from(embedded));
            }
            if in_v6(
                value,
                u128::from(Ipv6Addr::new(0x2002, 0, 0, 0, 0, 0, 0, 0)),
                16,
            ) {
                let embedded = u32::try_from((value >> 80) & u128::from(u32::MAX))
                    .expect("6to4 IPv4 field is bounded to u32");
                return denied_v4(Ipv4Addr::from(embedded));
            }
            IPV6_DENY
                .iter()
                .find(|(_, base, prefix)| in_v6(value, *base, *prefix))
                .map(|(name, _, _)| *name)
        }
    }
}

fn denied_v4(address: Ipv4Addr) -> Option<&'static str> {
    let value = u32::from(address);
    IPV4_DENY
        .iter()
        .find(|(_, base, prefix)| in_v4(value, *base, *prefix))
        .map(|(name, _, _)| *name)
}

const fn in_v4(value: u32, base: u32, prefix: u8) -> bool {
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    value & mask == base & mask
}

const fn in_v6(value: u128, base: u128, prefix: u8) -> bool {
    let mask = if prefix == 0 {
        0
    } else {
        u128::MAX << (128 - prefix)
    };
    value & mask == base & mask
}

const fn v4(a: u8, b: u8, c: u8, d: u8) -> u32 {
    u32::from_be_bytes([a, b, c, d])
}

const IPV4_DENY: &[(&str, u32, u8)] = &[
    ("0.0.0.0/8", v4(0, 0, 0, 0), 8),
    ("10.0.0.0/8", v4(10, 0, 0, 0), 8),
    ("100.64.0.0/10", v4(100, 64, 0, 0), 10),
    ("127.0.0.0/8", v4(127, 0, 0, 0), 8),
    ("169.254.0.0/16", v4(169, 254, 0, 0), 16),
    ("172.16.0.0/12", v4(172, 16, 0, 0), 12),
    ("192.0.0.0/24", v4(192, 0, 0, 0), 24),
    ("192.0.2.0/24", v4(192, 0, 2, 0), 24),
    ("192.88.99.0/24", v4(192, 88, 99, 0), 24),
    ("192.168.0.0/16", v4(192, 168, 0, 0), 16),
    ("198.18.0.0/15", v4(198, 18, 0, 0), 15),
    ("198.51.100.0/24", v4(198, 51, 100, 0), 24),
    ("203.0.113.0/24", v4(203, 0, 113, 0), 24),
    ("224.0.0.0/4", v4(224, 0, 0, 0), 4),
    ("240.0.0.0/4", v4(240, 0, 0, 0), 4),
    ("255.255.255.255/32", v4(255, 255, 255, 255), 32),
];

const IPV6_DENY: &[(&str, u128, u8)] = &[
    ("::/128", 0, 128),
    ("::1/128", 1, 128),
    ("100::/64", 0x0100_0000_0000_0000_0000_0000_0000_0000, 64),
    ("2001::/32", 0x2001_0000_0000_0000_0000_0000_0000_0000, 32),
    ("2001:2::/48", 0x2001_0002_0000_0000_0000_0000_0000_0000, 48),
    (
        "2001:db8::/32",
        0x2001_0db8_0000_0000_0000_0000_0000_0000,
        32,
    ),
    ("fc00::/7", 0xfc00_0000_0000_0000_0000_0000_0000_0000, 7),
    ("fe80::/10", 0xfe80_0000_0000_0000_0000_0000_0000_0000, 10),
    ("ff00::/8", 0xff00_0000_0000_0000_0000_0000_0000_0000, 8),
];

/// Why a Brain-originated target was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EgressRejection {
    /// URL was relative or malformed.
    #[error("egress URL must be absolute")]
    NotAbsolute,
    /// Scheme was outside the closed allowlist.
    #[error("egress URL scheme `{scheme}` is not allowed")]
    SchemeNotAllowed {
        /// Observed scheme.
        scheme: Box<str>,
    },
    /// Effective port was not 443.
    #[error("egress port {port} is not allowed")]
    PortNotAllowed {
        /// Observed port.
        port: u16,
    },
    /// URL embedded userinfo.
    #[error("egress URLs may not contain credentials")]
    CredentialsInUrl,
    /// A denied IP literal bypassed DNS.
    #[error("IP literal {addr} belongs to denied range {range}")]
    HostIsIpLiteralDenied {
        /// Literal address.
        addr: IpAddr,
        /// Matched range.
        range: &'static str,
    },
    /// A named local/metadata host was used.
    #[error("host `{host}` is local or metadata authority")]
    PrivateHostName {
        /// Refused name.
        host: String,
    },
    /// DNS returned no records.
    #[error("egress DNS resolution was empty")]
    EmptyResolution,
    /// Every returned record was denied.
    #[error("resolved address {addr} belongs to denied range {range}")]
    PrivateAddress {
        /// First denied address.
        addr: IpAddr,
        /// Matched range.
        range: &'static str,
    },
    /// DNS mixed public and denied records.
    #[error("egress DNS resolution mixed public and private addresses")]
    MixedPublicPrivate,
    /// Resolution exceeded the screening bound.
    #[error("egress DNS returned {n} addresses; maximum is 16")]
    TooManyAddresses {
        /// Observed address count.
        n: usize,
    },
    /// URL exceeded the public byte ceiling.
    #[error("egress URL is {bytes} bytes; maximum is 8192")]
    UrlTooLong {
        /// Observed byte length.
        bytes: usize,
    },
    /// Resolver failed or its blocking task could not complete.
    #[error("egress DNS resolution failed")]
    DnsFailure,
    /// A redirect response did not include a usable Location header.
    #[error("egress redirect is missing a valid Location header")]
    RedirectMissingLocation,
    /// A redirect repeated a previously visited absolute URL.
    #[error("egress redirect loop detected")]
    RedirectLoop,
    /// A redirect chain exceeded the configured ceiling.
    #[error("egress redirect limit exceeded")]
    RedirectLimit,
}
