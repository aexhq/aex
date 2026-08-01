//! The redaction newtype every presigned URL is wrapped in.
//!
//! A presigned URL is a bearer credential. Logging one is not a policy mistake
//! that review catches, it is an authorization leak, so the type simply cannot
//! print itself: `Debug` and `Display` both render a stable fingerprint. Making
//! the leak structurally impossible is the point — a redaction convention is
//! only as good as the last person who remembered it.

use std::fmt;

/// A presigned URL that will not print itself.
#[derive(Clone, PartialEq, Eq)]
pub struct RedactedUrl {
    url: String,
    fingerprint: String,
}

impl RedactedUrl {
    /// Wraps a presigned URL.
    #[must_use]
    pub fn new(url: impl Into<String>) -> Self {
        use sha2::Digest as _;
        let url = url.into();
        let mut hasher = sha2::Sha256::new();
        hasher.update(url.as_bytes());
        let digest = hex::encode(hasher.finalize());
        Self {
            fingerprint: digest[..8].to_owned(),
            url,
        }
    }

    /// The URL, for the one place that has to serialize it onto the wire.
    ///
    /// Named `expose` rather than `as_str` so a call site that leaks it is
    /// visible in a diff.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.url
    }

    /// The stable fingerprint, which is what a log or a span may carry.
    #[must_use]
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }
}

impl fmt::Debug for RedactedUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "<presigned-url:{}>", self.fingerprint)
    }
}

impl fmt::Display for RedactedUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "<presigned-url:{}>", self.fingerprint)
    }
}

#[cfg(test)]
mod tests {
    use super::RedactedUrl;

    const URL: &str = "https://bucket.s3.eu-west-1.amazonaws.com/ws_1/ab/cd/abcd\
                       ?X-Amz-Signature=deadbeefdeadbeefdeadbeefdeadbeef&X-Amz-Expires=300";

    #[test]
    fn neither_debug_nor_display_ever_prints_the_signature() {
        let redacted = RedactedUrl::new(URL);
        for rendering in [format!("{redacted:?}"), format!("{redacted}")] {
            assert!(!rendering.contains("X-Amz-Signature"), "{rendering}");
            assert!(!rendering.contains("deadbeef"), "{rendering}");
            assert!(!rendering.contains("amazonaws"), "{rendering}");
            assert!(rendering.starts_with("<presigned-url:"), "{rendering}");
        }
    }

    #[test]
    fn the_fingerprint_is_stable_and_distinguishes_two_urls() {
        assert_eq!(
            RedactedUrl::new(URL).fingerprint(),
            RedactedUrl::new(URL).fingerprint()
        );
        assert_ne!(
            RedactedUrl::new(URL).fingerprint(),
            RedactedUrl::new(format!("{URL}&extra=1")).fingerprint()
        );
        assert_eq!(RedactedUrl::new(URL).fingerprint().len(), 8);
    }

    #[test]
    fn the_url_is_reachable_only_through_a_call_that_names_the_exposure() {
        assert_eq!(RedactedUrl::new(URL).expose(), URL);
    }
}
