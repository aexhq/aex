//! Hosted customer-Hand gateway admission.
//!
//! API Gateway supplies connection metadata to one HTTP proxy target. This module authenticates
//! that integration and rejects managed-sandbox sources before any frame reaches Brain. Brain owns
//! connection epochs, registration and frame semantics; Aex does not duplicate that protocol.

use std::net::{IpAddr, SocketAddr};

use axum::http::HeaderMap;
use ipnet::IpNet;

use crate::identity;
use crate::{Error, Result};

const TOKEN_HEADER: &str = "x-aex-apigateway-token";
const CONNECTION_HEADER: &str = "x-aex-connection-id";
const ROUTE_HEADER: &str = "x-aex-route-key";
const REQUEST_HEADER: &str = "x-aex-request-id";
const SOURCE_HEADER: &str = "x-aex-source-ip";
const WEBSOCKET_PROTOCOL_HEADER: &str = "sec-websocket-protocol";

#[derive(Clone)]
pub struct CustomerHandGateway {
    token_hash: String,
    trusted_proxy_cidrs: Vec<IpNet>,
    blocked_source_cidrs: Vec<IpNet>,
}

impl std::fmt::Debug for CustomerHandGateway {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CustomerHandGateway")
            .field("token_hash", &"<redacted>")
            .field("trusted_proxy_cidrs", &self.trusted_proxy_cidrs)
            .field("blocked_source_cidrs", &self.blocked_source_cidrs)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayRequest {
    pub connection_id: String,
    pub route: GatewayRoute,
    pub request_id: String,
    pub source_ip: IpAddr,
    pub protocol: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatewayRoute {
    Connect,
    Disconnect,
    Message,
}

impl GatewayRoute {
    pub fn as_brain_header(self) -> &'static str {
        match self {
            Self::Connect => "$connect",
            Self::Disconnect => "$disconnect",
            Self::Message => "$default",
        }
    }
}

impl CustomerHandGateway {
    pub fn new(
        token: &str,
        trusted_proxy_cidrs: Vec<IpNet>,
        blocked_source_cidrs: Vec<IpNet>,
    ) -> Result<Self> {
        if token.len() < 32
            || token.len() > 256
            || !token.bytes().all(|byte| byte.is_ascii_graphic())
        {
            return Err(Error::Invalid(
                "customer-Hand gateway token must contain 32 through 256 visible ASCII bytes"
                    .into(),
            ));
        }
        if trusted_proxy_cidrs.is_empty() {
            return Err(Error::Invalid(
                "customer-Hand gateway requires at least one trusted proxy CIDR".into(),
            ));
        }
        if blocked_source_cidrs.is_empty() {
            return Err(Error::Invalid(
                "customer-Hand gateway requires managed-sandbox NAT CIDRs".into(),
            ));
        }
        Ok(Self {
            token_hash: identity::hash_secret(token),
            trusted_proxy_cidrs,
            blocked_source_cidrs,
        })
    }

    pub fn authenticate(&self, headers: &HeaderMap, peer: SocketAddr) -> Result<GatewayRequest> {
        if !self
            .trusted_proxy_cidrs
            .iter()
            .any(|network| network.contains(&peer.ip()))
        {
            return Err(Error::Forbidden(
                "customer-Hand gateway request did not arrive through a trusted proxy".into(),
            ));
        }
        let connection_id = bounded_identity(
            required_header(headers, CONNECTION_HEADER)?,
            "connection id",
        )?;
        let request_id = bounded_identity(required_header(headers, REQUEST_HEADER)?, "request id")?;
        let route = match required_header(headers, ROUTE_HEADER)? {
            "$connect" => GatewayRoute::Connect,
            "$disconnect" => GatewayRoute::Disconnect,
            "$default" => GatewayRoute::Message,
            _ => return Err(Error::Invalid("unknown customer-Hand gateway route".into())),
        };
        // API Gateway authorizer context is reliable only during `$connect`. Later frames carry a
        // connection-bound proof in Brain's raw protocol and Brain verifies it before mutation.
        // If a token is supplied on a later route it must still be the real one; callers cannot
        // use an explicitly bad credential to fall back to the proof-only path.
        let integration_token = headers
            .get(TOKEN_HEADER)
            .and_then(|value| value.to_str().ok())
            .filter(|value| !value.is_empty());
        if matches!(route, GatewayRoute::Connect) && integration_token.is_none() {
            return Err(Error::Unauthorized);
        }
        if integration_token
            .is_some_and(|token| !identity::secret_matches_hash(token, &self.token_hash))
        {
            return Err(Error::Unauthorized);
        }
        let protocol = match route {
            GatewayRoute::Connect => Some(single_protocol(headers)?),
            GatewayRoute::Disconnect | GatewayRoute::Message => None,
        };
        let source_ip = required_header(headers, SOURCE_HEADER)?
            .parse::<IpAddr>()
            .map_err(|_| Error::Invalid("invalid customer-Hand source IP".into()))?;
        self.reject_blocked(source_ip)?;

        // ALB appends the immediate source to X-Forwarded-For. Inspect every parseable address:
        // a caller can only add values before ALB's value, and adding a blocked value merely rejects
        // its own request. The API Gateway context address above remains the end-user authority.
        let forwarded = required_header(headers, "x-forwarded-for")?;
        let mut forwarded_addresses = 0usize;
        for value in forwarded
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            let address = value
                .parse::<IpAddr>()
                .map_err(|_| Error::Invalid("invalid X-Forwarded-For address".into()))?;
            forwarded_addresses += 1;
            self.reject_blocked(address)?;
        }
        if forwarded_addresses == 0 {
            return Err(Error::Invalid("X-Forwarded-For contains no address".into()));
        }

        Ok(GatewayRequest {
            connection_id,
            route,
            request_id,
            source_ip,
            protocol,
        })
    }

    fn reject_blocked(&self, address: IpAddr) -> Result<()> {
        if self
            .blocked_source_cidrs
            .iter()
            .any(|network| network.contains(&address))
        {
            return Err(Error::Forbidden(
                "managed sandbox network identities cannot attach a customer Hand".into(),
            ));
        }
        Ok(())
    }
}

fn single_protocol(headers: &HeaderMap) -> Result<String> {
    if headers
        .get_all(WEBSOCKET_PROTOCOL_HEADER)
        .iter()
        .take(2)
        .count()
        != 1
    {
        return Err(Error::Invalid(
            "customer-Hand connection must select exactly one valid WebSocket grant protocol"
                .into(),
        ));
    }
    let value = required_header(headers, WEBSOCKET_PROTOCOL_HEADER)?;
    if value.len() > 2048
        || value.contains(',')
        || !value.bytes().all(|byte| byte.is_ascii_graphic())
    {
        return Err(Error::Invalid(
            "customer-Hand connection must select exactly one valid WebSocket grant protocol"
                .into(),
        ));
    }
    Ok(value.to_owned())
}

pub fn parse_cidrs(name: &str, value: &str) -> anyhow::Result<Vec<IpNet>> {
    let mut networks = Vec::new();
    for item in value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
    {
        let network = item
            .parse::<IpNet>()
            .map_err(|error| anyhow::anyhow!("{name} contains invalid CIDR {item:?}: {error}"))?;
        if networks.contains(&network) {
            anyhow::bail!("{name} repeats CIDR {network}");
        }
        networks.push(network);
    }
    Ok(networks)
}

fn required_header<'a>(headers: &'a HeaderMap, name: &str) -> Result<&'a str> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Error::Invalid(format!("missing or invalid {name} header")))
}

fn bounded_identity(value: &str, label: &str) -> Result<String> {
    if value.len() > 256 || !value.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(Error::Invalid(format!("invalid customer-Hand {label}")));
    }
    Ok(value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn gateway() -> CustomerHandGateway {
        CustomerHandGateway::new(
            "gateway-secret-with-at-least-thirty-two-bytes",
            vec!["127.0.0.0/8".parse().unwrap()],
            vec!["198.51.100.0/24".parse().unwrap()],
        )
        .unwrap()
    }

    fn headers(source: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in [
            (
                TOKEN_HEADER,
                "gateway-secret-with-at-least-thirty-two-bytes",
            ),
            (CONNECTION_HEADER, "connection-1"),
            (ROUTE_HEADER, "$connect"),
            (REQUEST_HEADER, "request-1"),
            (SOURCE_HEADER, source),
            (WEBSOCKET_PROTOCOL_HEADER, "aex.grant.test"),
            ("x-forwarded-for", "203.0.113.7"),
        ] {
            headers.insert(name, HeaderValue::from_str(value).unwrap());
        }
        headers
    }

    #[test]
    fn authenticates_injected_metadata_through_a_trusted_proxy() {
        let request = gateway()
            .authenticate(&headers("203.0.113.7"), "127.0.0.1:1234".parse().unwrap())
            .unwrap();
        assert_eq!(request.route, GatewayRoute::Connect);
        assert_eq!(request.source_ip, "203.0.113.7".parse::<IpAddr>().unwrap());
        assert_eq!(request.protocol.as_deref(), Some("aex.grant.test"));
    }

    #[test]
    fn blocks_managed_nat_in_context_or_forwarding_chain() {
        assert!(
            gateway()
                .authenticate(&headers("198.51.100.8"), "127.0.0.1:1234".parse().unwrap())
                .is_err()
        );
        let mut forwarded = headers("203.0.113.7");
        forwarded.insert(
            "x-forwarded-for",
            HeaderValue::from_static("203.0.113.7, 198.51.100.8"),
        );
        assert!(
            gateway()
                .authenticate(&forwarded, "127.0.0.1:1234".parse().unwrap())
                .is_err()
        );
    }

    #[test]
    fn rejects_direct_or_unauthenticated_requests() {
        let headers = headers("203.0.113.7");
        assert!(
            gateway()
                .authenticate(&headers, "10.0.0.9:1234".parse().unwrap())
                .is_err()
        );
        let mut missing_token = headers;
        missing_token.remove(TOKEN_HEADER);
        assert!(
            gateway()
                .authenticate(&missing_token, "127.0.0.1:1234".parse().unwrap())
                .is_err()
        );
    }

    #[test]
    fn later_frames_rely_on_brains_bound_proof_when_authorizer_context_is_absent() {
        let mut message = headers("203.0.113.7");
        message.insert(ROUTE_HEADER, HeaderValue::from_static("$default"));
        message.remove(WEBSOCKET_PROTOCOL_HEADER);
        message.remove(TOKEN_HEADER);
        assert_eq!(
            gateway()
                .authenticate(&message, "127.0.0.1:1234".parse().unwrap())
                .unwrap()
                .route,
            GatewayRoute::Message
        );

        message.insert(TOKEN_HEADER, HeaderValue::from_static("explicitly-wrong"));
        assert!(
            gateway()
                .authenticate(&message, "127.0.0.1:1234".parse().unwrap())
                .is_err()
        );
    }

    #[test]
    fn connect_requires_exactly_one_websocket_grant_protocol() {
        let mut missing = headers("203.0.113.7");
        missing.remove(WEBSOCKET_PROTOCOL_HEADER);
        assert!(
            gateway()
                .authenticate(&missing, "127.0.0.1:1234".parse().unwrap())
                .is_err()
        );
        let mut multiple = headers("203.0.113.7");
        multiple.insert(
            WEBSOCKET_PROTOCOL_HEADER,
            HeaderValue::from_static("aex.grant.one, aex.grant.two"),
        );
        assert!(
            gateway()
                .authenticate(&multiple, "127.0.0.1:1234".parse().unwrap())
                .is_err()
        );
        let mut repeated = headers("203.0.113.7");
        repeated.append(
            WEBSOCKET_PROTOCOL_HEADER,
            HeaderValue::from_static("aex.grant.two"),
        );
        assert!(
            gateway()
                .authenticate(&repeated, "127.0.0.1:1234".parse().unwrap())
                .is_err()
        );
    }
}
