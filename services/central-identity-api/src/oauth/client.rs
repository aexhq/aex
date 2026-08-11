//! OAuth client configuration and secret loading.

use serde::Deserialize;

/// One provider's registered OAuth client.
///
/// The secret is held as a `String` because a token endpoint needs it in
/// plaintext at every exchange, and it is kept out of every rendering this type
/// can produce.
#[derive(Clone, PartialEq, Eq)]
pub struct OauthClient {
    id: String,
    pub(super) secret: String,
}

impl std::fmt::Debug for OauthClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OauthClient")
            .field("id", &self.id)
            .field("secret", &"<redacted>")
            .finish()
    }
}

/// Why a configured OAuth client was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum OauthClientError {
    /// The client id was absent or blank.
    #[error("the OAuth client id is blank")]
    BlankId,
    /// The client secret was absent or blank.
    #[error("the OAuth client secret is blank")]
    BlankSecret,
}

impl OauthClient {
    /// Validates a configured client.
    ///
    /// # Errors
    ///
    /// Returns [`OauthClientError`] when either half is blank. A blank secret
    /// would produce an exchange that fails on every sign-in for the life of
    /// the process, and start-up is the only place refusing it costs nothing.
    pub fn new(id: &str, secret: &str) -> Result<Self, OauthClientError> {
        let id = id.trim();
        let secret = secret.trim();
        if id.is_empty() {
            return Err(OauthClientError::BlankId);
        }
        if secret.is_empty() {
            return Err(OauthClientError::BlankSecret);
        }
        Ok(Self {
            id: id.to_owned(),
            secret: secret.to_owned(),
        })
    }

    /// The public client identifier.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }
}

/// The shape a provider's OAuth client secret is stored in.
///
/// A JSON envelope, unlike the peppers this crate loads whole, because there are
/// genuinely two values and Secrets Manager stores a key/value secret natively.
/// One rotation writes both halves or neither.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredClient {
    client_id: String,
    client_secret: String,
}

/// Reads one provider's registered OAuth client from Secrets Manager.
///
/// # Errors
///
/// Returns the reason as a string, for the caller to name its own dependency
/// with. Nothing in the message can carry the secret: a parse failure names the
/// shape, never the value.
pub async fn load_oauth_client(
    secrets: &aws_sdk_secretsmanager::Client,
    secret_id: &str,
) -> Result<OauthClient, String> {
    let value = secrets
        .get_secret_value()
        .secret_id(secret_id)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let plaintext = value
        .secret_string()
        .ok_or_else(|| "the secret holds no string value".to_owned())?;
    let stored: StoredClient = serde_json::from_str(plaintext).map_err(|_| {
        "the secret is not a JSON object with `clientId` and `clientSecret`".to_owned()
    })?;
    OauthClient::new(&stored.client_id, &stored.client_secret).map_err(|error| error.to_string())
}
