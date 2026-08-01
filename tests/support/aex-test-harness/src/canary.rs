//! The run's secret canary and the leak scanner that hunts for it.
//!
//! A run mints one canary, injects it wherever a platform secret would flow,
//! and then scans every log line, span attribute, `JUnit` body, exported object,
//! receipt and artifact for it. A hit fails the run.
//!
//! The canary value is never printed, not in a log, not in a failure message,
//! not in a test name. [`LeakFinding`] therefore carries the shape and the
//! position and nothing else - reporting the value in the report that proves it
//! leaked would be the same defect.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// 32 random bytes, rendered as 64 lowercase hex characters.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretCanary(String);

impl SecretCanary {
    /// The number of hex characters a canary renders to.
    pub const HEX_LEN: usize = 64;

    /// Mints a new canary.
    #[must_use]
    pub fn mint() -> Self {
        let mut bytes = [0_u8; 32];
        bytes[..16].copy_from_slice(Uuid::new_v4().as_bytes());
        bytes[16..].copy_from_slice(Uuid::new_v4().as_bytes());
        Self(hex::encode(bytes))
    }

    /// The value, for injection into the system under test.
    ///
    /// Every other accessor deliberately hides it; this one is named so a
    /// review notices the call site.
    #[must_use]
    pub fn expose_for_injection(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for SecretCanary {
    /// Redacted, so a `dbg!` or a panic payload cannot leak the canary that
    /// exists to detect leaks.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SecretCanary(<redacted>)")
    }
}

/// Which shape was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeakShape {
    /// This run's own canary.
    RunCanary,
    /// An AWS access key id.
    AwsAccessKeyId,
    /// A provider secret key.
    ProviderSecretKey,
    /// A webhook signing secret.
    WebhookSigningSecret,
    /// A PEM private key block.
    PrivateKeyBlock,
    /// A bearer token of at least 32 characters.
    BearerToken,
}

impl LeakShape {
    /// The shape's name as it appears in the failure message.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RunCanary => "run canary",
            Self::AwsAccessKeyId => "aws access key id",
            Self::ProviderSecretKey => "provider secret key",
            Self::WebhookSigningSecret => "webhook signing secret",
            Self::PrivateKeyBlock => "private key block",
            Self::BearerToken => "bearer token",
        }
    }
}

/// Where a leak was found. Never what it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct LeakFinding {
    /// The shape that matched.
    pub shape: LeakShape,
    /// One-based line number.
    pub line: usize,
    /// One-based byte column within the line.
    pub column: usize,
}

impl std::fmt::Display for LeakFinding {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{} at line {} column {}",
            self.shape.as_str(),
            self.line,
            self.column
        )
    }
}

/// Scans `haystack` for the run canary and for every static secret shape.
///
/// Findings are ordered by position, so a diff of two scans is readable.
#[must_use]
pub fn scan_for_leaks(haystack: &str, canary: Option<&SecretCanary>) -> Vec<LeakFinding> {
    let mut findings = Vec::new();
    for (index, text) in haystack.lines().enumerate() {
        let line = index + 1;
        if let Some(canary) = canary {
            let mut from = 0;
            while let Some(offset) = text[from..].find(canary.expose_for_injection()) {
                let column = from + offset;
                findings.push(LeakFinding {
                    shape: LeakShape::RunCanary,
                    line,
                    column: column + 1,
                });
                from = column + 1;
            }
        }
        findings.extend(scan_line(text, line));
    }
    findings.sort_unstable();
    findings.dedup();
    findings
}

fn scan_line(text: &str, line: usize) -> Vec<LeakFinding> {
    let bytes = text.as_bytes();
    let mut findings = Vec::new();
    let mut push = |shape: LeakShape, column: usize| {
        findings.push(LeakFinding {
            shape,
            line,
            column: column + 1,
        });
    };

    for start in 0..bytes.len() {
        if text[start..].starts_with("AKIA")
            && bytes.get(start + 4..start + 20).is_some_and(|tail| {
                tail.iter()
                    .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
            })
        {
            push(LeakShape::AwsAccessKeyId, start);
        }
        if text[start..].starts_with("sk-") {
            let run = bytes[start + 3..]
                .iter()
                .take_while(|byte| byte.is_ascii_alphanumeric())
                .count();
            if run >= 20 {
                push(LeakShape::ProviderSecretKey, start);
            }
        }
        if text[start..].starts_with("whsec_") {
            push(LeakShape::WebhookSigningSecret, start);
        }
        if text[start..].starts_with("-----BEGIN") && text[start..].contains("PRIVATE KEY-----") {
            push(LeakShape::PrivateKeyBlock, start);
        }
        if text[start..].starts_with("Bearer ") {
            let run = bytes[start + 7..]
                .iter()
                .take_while(|byte| !byte.is_ascii_whitespace())
                .count();
            if run >= 32 {
                push(LeakShape::BearerToken, start);
            }
        }
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::{LeakShape, SecretCanary, scan_for_leaks};

    #[test]
    fn a_minted_canary_is_64_hex_characters_and_never_debug_printed() {
        let canary = SecretCanary::mint();
        assert_eq!(canary.expose_for_injection().len(), SecretCanary::HEX_LEN);
        assert!(
            canary
                .expose_for_injection()
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        );
        let rendered = format!("{canary:?}");
        assert_eq!(rendered, "SecretCanary(<redacted>)");
        assert!(!rendered.contains(canary.expose_for_injection()));
    }

    #[test]
    fn the_canary_is_found_wherever_it_appears_and_never_echoed() {
        let canary = SecretCanary::mint();
        let text = format!(
            "line one\nprefix {} suffix\nline three",
            canary.expose_for_injection()
        );
        let findings = scan_for_leaks(&text, Some(&canary));
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].shape, LeakShape::RunCanary);
        assert_eq!(findings[0].line, 2);
        assert_eq!(findings[0].column, 8);
        let rendered = findings[0].to_string();
        assert!(
            !rendered.contains(canary.expose_for_injection()),
            "{rendered}"
        );
    }

    #[test]
    fn every_static_shape_is_recognised() {
        let cases = [
            (
                format!("{}{}", "AKIA", "ABCDEFGHIJKLMNOP"),
                LeakShape::AwsAccessKeyId,
            ),
            (
                format!("{}{}", "sk-", "a".repeat(24)),
                LeakShape::ProviderSecretKey,
            ),
            (
                format!("{}{}", "whsec_", "deadbeef"),
                LeakShape::WebhookSigningSecret,
            ),
            (
                format!("{}{}", "-----BEGIN RSA ", "PRIVATE KEY-----"),
                LeakShape::PrivateKeyBlock,
            ),
            (
                format!("{}{}", "Bearer ", "z".repeat(40)),
                LeakShape::BearerToken,
            ),
        ];
        for (text, shape) in cases {
            let findings = scan_for_leaks(&text, None);
            assert!(
                findings.iter().any(|finding| finding.shape == shape),
                "{shape:?} not found in a line of length {}",
                text.len()
            );
        }
    }

    #[test]
    fn a_short_token_is_not_a_leak() {
        assert!(scan_for_leaks(&format!("{}{}", "sk-", "a".repeat(10)), None).is_empty());
        assert!(scan_for_leaks(&format!("{}{}", "Bearer ", "z".repeat(8)), None).is_empty());
        assert!(scan_for_leaks(&format!("{}{}", "AKIA", "short"), None).is_empty());
    }

    #[test]
    fn ordinary_text_produces_no_finding() {
        assert!(scan_for_leaks("the quick brown fox\njumps over 42 lazy dogs", None).is_empty());
    }
}
