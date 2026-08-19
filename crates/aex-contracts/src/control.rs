#![allow(clippy::redundant_closure_call)]
#![allow(clippy::needless_lifetimes)]
#![allow(clippy::match_single_binding)]
#![allow(clippy::clone_on_copy)]

#[doc = r" Error types."]
pub mod error {
    #[doc = r" Error from a `TryFrom` or `FromStr` implementation."]
    pub struct ConversionError(::std::borrow::Cow<'static, str>);
    impl ::std::error::Error for ConversionError {}
    impl ::std::fmt::Display for ConversionError {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> Result<(), ::std::fmt::Error> {
            ::std::fmt::Display::fmt(&self.0, f)
        }
    }
    impl ::std::fmt::Debug for ConversionError {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> Result<(), ::std::fmt::Error> {
            ::std::fmt::Debug::fmt(&self.0, f)
        }
    }
    impl From<&'static str> for ConversionError {
        fn from(value: &'static str) -> Self {
            Self(value.into())
        }
    }
    impl From<String> for ConversionError {
        fn from(value: String) -> Self {
            Self(value.into())
        }
    }
}
#[doc = "`Account`"]
#[doc = r""]
#[doc = r" <details><summary>JSON schema</summary>"]
#[doc = r""]
#[doc = r" ```json"]
#[doc = "{"]
#[doc = "  \"type\": \"object\","]
#[doc = "  \"required\": ["]
#[doc = "    \"created_at\","]
#[doc = "    \"email\","]
#[doc = "    \"id\","]
#[doc = "    \"limits\","]
#[doc = "    \"object\""]
#[doc = "  ],"]
#[doc = "  \"properties\": {"]
#[doc = "    \"created_at\": {"]
#[doc = "      \"$ref\": \"#/$defs/Timestamp\""]
#[doc = "    },"]
#[doc = "    \"email\": {"]
#[doc = "      \"type\": \"string\""]
#[doc = "    },"]
#[doc = "    \"id\": {"]
#[doc = "      \"$ref\": \"#/$defs/AccountId\""]
#[doc = "    },"]
#[doc = "    \"limits\": {"]
#[doc = "      \"$ref\": \"#/$defs/AccountLimits\""]
#[doc = "    },"]
#[doc = "    \"object\": {"]
#[doc = "      \"const\": \"account\""]
#[doc = "    }"]
#[doc = "  }"]
#[doc = "}"]
#[doc = r" ```"]
#[doc = r" </details>"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, PartialEq)]
pub struct Account {
    pub created_at: Timestamp,
    pub email: ::std::string::String,
    pub id: AccountId,
    pub limits: AccountLimits,
    pub object: ::serde_json::Value,
}
impl Account {
    pub fn builder() -> builder::Account {
        Default::default()
    }
}
#[doc = "The account token appears here and never again."]
#[doc = r""]
#[doc = r" <details><summary>JSON schema</summary>"]
#[doc = r""]
#[doc = r" ```json"]
#[doc = "{"]
#[doc = "  \"description\": \"The account token appears here and never again.\","]
#[doc = "  \"type\": \"object\","]
#[doc = "  \"required\": ["]
#[doc = "    \"account\","]
#[doc = "    \"account_token\""]
#[doc = "  ],"]
#[doc = "  \"properties\": {"]
#[doc = "    \"account\": {"]
#[doc = "      \"$ref\": \"#/$defs/Account\""]
#[doc = "    },"]
#[doc = "    \"account_token\": {"]
#[doc = "      \"$ref\": \"#/$defs/AccountToken\""]
#[doc = "    }"]
#[doc = "  }"]
#[doc = "}"]
#[doc = r" ```"]
#[doc = r" </details>"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, PartialEq)]
pub struct AccountCreated {
    pub account: Account,
    pub account_token: AccountToken,
}
impl AccountCreated {
    pub fn builder() -> builder::AccountCreated {
        Default::default()
    }
}
#[doc = "`AccountId`"]
#[doc = r""]
#[doc = r" <details><summary>JSON schema</summary>"]
#[doc = r""]
#[doc = r" ```json"]
#[doc = "{"]
#[doc = "  \"type\": \"string\","]
#[doc = "  \"pattern\": \"^acc_[A-Za-z0-9]{20,32}$\""]
#[doc = "}"]
#[doc = r" ```"]
#[doc = r" </details>"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct AccountId(::std::string::String);
impl ::std::ops::Deref for AccountId {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<AccountId> for ::std::string::String {
    fn from(value: AccountId) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for AccountId {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| {
                ::regress::Regex::new("^acc_[A-Za-z0-9]{20,32}$").unwrap()
            });
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^acc_[A-Za-z0-9]{20,32}$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for AccountId {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<&::std::string::String> for AccountId {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for AccountId {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for AccountId {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "Abuse controls (ARCHITECTURE-v1 §2.9): card + minimum top-up, concurrency and create-rate caps."]
#[doc = r""]
#[doc = r" <details><summary>JSON schema</summary>"]
#[doc = r""]
#[doc = r" ```json"]
#[doc = "{"]
#[doc = "  \"description\": \"Abuse controls (ARCHITECTURE-v1 §2.9): card + minimum top-up, concurrency and create-rate caps.\","]
#[doc = "  \"type\": \"object\","]
#[doc = "  \"required\": ["]
#[doc = "    \"max_concurrent_sessions\","]
#[doc = "    \"session_creates_per_hour\""]
#[doc = "  ],"]
#[doc = "  \"properties\": {"]
#[doc = "    \"max_concurrent_sessions\": {"]
#[doc = "      \"type\": \"integer\","]
#[doc = "      \"minimum\": 1.0"]
#[doc = "    },"]
#[doc = "    \"session_creates_per_hour\": {"]
#[doc = "      \"type\": \"integer\","]
#[doc = "      \"minimum\": 1.0"]
#[doc = "    }"]
#[doc = "  }"]
#[doc = "}"]
#[doc = r" ```"]
#[doc = r" </details>"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, PartialEq)]
pub struct AccountLimits {
    pub max_concurrent_sessions: ::std::num::NonZeroU64,
    pub session_creates_per_hour: ::std::num::NonZeroU64,
}
impl AccountLimits {
    pub fn builder() -> builder::AccountLimits {
        Default::default()
    }
}
#[doc = "Manages the account: keys, top-ups, the bill. Shown once at signup; only a hash is stored."]
#[doc = r""]
#[doc = r" <details><summary>JSON schema</summary>"]
#[doc = r""]
#[doc = r" ```json"]
#[doc = "{"]
#[doc = "  \"description\": \"Manages the account: keys, top-ups, the bill. Shown once at signup; only a hash is stored.\","]
#[doc = "  \"type\": \"string\","]
#[doc = "  \"pattern\": \"^aex_at_[A-Za-z0-9]{40,64}$\""]
#[doc = "}"]
#[doc = r" ```"]
#[doc = r" </details>"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct AccountToken(::std::string::String);
impl ::std::ops::Deref for AccountToken {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<AccountToken> for ::std::string::String {
    fn from(value: AccountToken) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for AccountToken {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| {
                ::regress::Regex::new("^aex_at_[A-Za-z0-9]{40,64}$").unwrap()
            });
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^aex_at_[A-Za-z0-9]{40,64}$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for AccountToken {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<&::std::string::String> for AccountToken {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for AccountToken {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for AccountToken {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "Component types of the control plane: identity (accounts, API keys), prepaid billing (top-ups, balance), and rated usage on the two-rate card. Paths are in openapi.yaml. Session operations are NOT redefined here: the control plane serves session/v1 paths verbatim, authorized by an API key, in front of a brain. All money on the wire is integer micro-USD (1 USD = 1,000,000 micro-USD) except top-up amounts, which are whole cents (the payment surface). Storage is metered in decimal GB (1 GB = 1e9 bytes); a month is 730 hours."]
#[doc = r""]
#[doc = r" <details><summary>JSON schema</summary>"]
#[doc = r""]
#[doc = r" ```json"]
#[doc = "{"]
#[doc = "  \"$id\": \"https://aex.dev/contracts/control/v1/schemas.json\","]
#[doc = "  \"title\": \"aex control API v1 types\","]
#[doc = "  \"description\": \"Component types of the control plane: identity (accounts, API keys), prepaid billing (top-ups, balance), and rated usage on the two-rate card. Paths are in openapi.yaml. Session operations are NOT redefined here: the control plane serves session/v1 paths verbatim, authorized by an API key, in front of a brain. All money on the wire is integer micro-USD (1 USD = 1,000,000 micro-USD) except top-up amounts, which are whole cents (the payment surface). Storage is metered in decimal GB (1 GB = 1e9 bytes); a month is 730 hours.\""]
#[doc = "}"]
#[doc = r" ```"]
#[doc = r" </details>"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, PartialEq)]
#[serde(transparent)]
pub struct AexControlApiV1Types(pub ::serde_json::Value);
impl ::std::ops::Deref for AexControlApiV1Types {
    type Target = ::serde_json::Value;
    fn deref(&self) -> &::serde_json::Value {
        &self.0
    }
}
impl ::std::convert::From<AexControlApiV1Types> for ::serde_json::Value {
    fn from(value: AexControlApiV1Types) -> Self {
        value.0
    }
}
impl ::std::convert::From<::serde_json::Value> for AexControlApiV1Types {
    fn from(value: ::serde_json::Value) -> Self {
        Self(value)
    }
}
#[doc = "`ApiKey`"]
#[doc = r""]
#[doc = r" <details><summary>JSON schema</summary>"]
#[doc = r""]
#[doc = r" ```json"]
#[doc = "{"]
#[doc = "  \"type\": \"object\","]
#[doc = "  \"required\": ["]
#[doc = "    \"created_at\","]
#[doc = "    \"id\","]
#[doc = "    \"name\","]
#[doc = "    \"object\","]
#[doc = "    \"prefix\""]
#[doc = "  ],"]
#[doc = "  \"properties\": {"]
#[doc = "    \"created_at\": {"]
#[doc = "      \"$ref\": \"#/$defs/Timestamp\""]
#[doc = "    },"]
#[doc = "    \"id\": {"]
#[doc = "      \"$ref\": \"#/$defs/KeyId\""]
#[doc = "    },"]
#[doc = "    \"last_used_at\": {"]
#[doc = "      \"$ref\": \"#/$defs/Timestamp\""]
#[doc = "    },"]
#[doc = "    \"name\": {"]
#[doc = "      \"type\": \"string\","]
#[doc = "      \"maxLength\": 128,"]
#[doc = "      \"minLength\": 1"]
#[doc = "    },"]
#[doc = "    \"object\": {"]
#[doc = "      \"const\": \"api_key\""]
#[doc = "    },"]
#[doc = "    \"prefix\": {"]
#[doc = "      \"description\": \"First characters of the secret, for recognising a key in a list. Never enough to authenticate.\","]
#[doc = "      \"type\": \"string\","]
#[doc = "      \"maxLength\": 20,"]
#[doc = "      \"minLength\": 10"]
#[doc = "    },"]
#[doc = "    \"revoked_at\": {"]
#[doc = "      \"$ref\": \"#/$defs/Timestamp\""]
#[doc = "    }"]
#[doc = "  }"]
#[doc = "}"]
#[doc = r" ```"]
#[doc = r" </details>"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, PartialEq)]
pub struct ApiKey {
    pub created_at: Timestamp,
    pub id: KeyId,
    #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
    pub last_used_at: ::std::option::Option<Timestamp>,
    pub name: ApiKeyName,
    pub object: ::serde_json::Value,
    #[doc = "First characters of the secret, for recognising a key in a list. Never enough to authenticate."]
    pub prefix: ApiKeyPrefix,
    #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
    pub revoked_at: ::std::option::Option<Timestamp>,
}
impl ApiKey {
    pub fn builder() -> builder::ApiKey {
        Default::default()
    }
}
#[doc = "The secret appears here and never again."]
#[doc = r""]
#[doc = r" <details><summary>JSON schema</summary>"]
#[doc = r""]
#[doc = r" ```json"]
#[doc = "{"]
#[doc = "  \"description\": \"The secret appears here and never again.\","]
#[doc = "  \"type\": \"object\","]
#[doc = "  \"required\": ["]
#[doc = "    \"key\","]
#[doc = "    \"secret\""]
#[doc = "  ],"]
#[doc = "  \"properties\": {"]
#[doc = "    \"key\": {"]
#[doc = "      \"$ref\": \"#/$defs/ApiKey\""]
#[doc = "    },"]
#[doc = "    \"secret\": {"]
#[doc = "      \"$ref\": \"#/$defs/ApiKeySecret\""]
#[doc = "    }"]
#[doc = "  }"]
#[doc = "}"]
#[doc = r" ```"]
#[doc = r" </details>"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, PartialEq)]
pub struct ApiKeyCreated {
    pub key: ApiKey,
    pub secret: ApiKeySecret,
}
impl ApiKeyCreated {
    pub fn builder() -> builder::ApiKeyCreated {
        Default::default()
    }
}
#[doc = "`ApiKeyList`"]
#[doc = r""]
#[doc = r" <details><summary>JSON schema</summary>"]
#[doc = r""]
#[doc = r" ```json"]
#[doc = "{"]
#[doc = "  \"type\": \"object\","]
#[doc = "  \"required\": ["]
#[doc = "    \"data\","]
#[doc = "    \"object\""]
#[doc = "  ],"]
#[doc = "  \"properties\": {"]
#[doc = "    \"data\": {"]
#[doc = "      \"type\": \"array\","]
#[doc = "      \"items\": {"]
#[doc = "        \"$ref\": \"#/$defs/ApiKey\""]
#[doc = "      }"]
#[doc = "    },"]
#[doc = "    \"object\": {"]
#[doc = "      \"const\": \"list\""]
#[doc = "    }"]
#[doc = "  }"]
#[doc = "}"]
#[doc = r" ```"]
#[doc = r" </details>"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, PartialEq)]
pub struct ApiKeyList {
    pub data: ::std::vec::Vec<ApiKey>,
    pub object: ::serde_json::Value,
}
impl ApiKeyList {
    pub fn builder() -> builder::ApiKeyList {
        Default::default()
    }
}
#[doc = "`ApiKeyName`"]
#[doc = r""]
#[doc = r" <details><summary>JSON schema</summary>"]
#[doc = r""]
#[doc = r" ```json"]
#[doc = "{"]
#[doc = "  \"type\": \"string\","]
#[doc = "  \"maxLength\": 128,"]
#[doc = "  \"minLength\": 1"]
#[doc = "}"]
#[doc = r" ```"]
#[doc = r" </details>"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct ApiKeyName(::std::string::String);
impl ::std::ops::Deref for ApiKeyName {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<ApiKeyName> for ::std::string::String {
    fn from(value: ApiKeyName) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for ApiKeyName {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        if value.chars().count() > 128usize {
            return Err("longer than 128 characters".into());
        }
        if value.chars().count() < 1usize {
            return Err("shorter than 1 characters".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for ApiKeyName {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<&::std::string::String> for ApiKeyName {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ApiKeyName {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for ApiKeyName {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "First characters of the secret, for recognising a key in a list. Never enough to authenticate."]
#[doc = r""]
#[doc = r" <details><summary>JSON schema</summary>"]
#[doc = r""]
#[doc = r" ```json"]
#[doc = "{"]
#[doc = "  \"description\": \"First characters of the secret, for recognising a key in a list. Never enough to authenticate.\","]
#[doc = "  \"type\": \"string\","]
#[doc = "  \"maxLength\": 20,"]
#[doc = "  \"minLength\": 10"]
#[doc = "}"]
#[doc = r" ```"]
#[doc = r" </details>"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct ApiKeyPrefix(::std::string::String);
impl ::std::ops::Deref for ApiKeyPrefix {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<ApiKeyPrefix> for ::std::string::String {
    fn from(value: ApiKeyPrefix) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for ApiKeyPrefix {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        if value.chars().count() > 20usize {
            return Err("longer than 20 characters".into());
        }
        if value.chars().count() < 10usize {
            return Err("shorter than 10 characters".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for ApiKeyPrefix {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<&::std::string::String> for ApiKeyPrefix {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ApiKeyPrefix {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for ApiKeyPrefix {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "Runs sessions. Shown once at creation; only a hash is stored."]
#[doc = r""]
#[doc = r" <details><summary>JSON schema</summary>"]
#[doc = r""]
#[doc = r" ```json"]
#[doc = "{"]
#[doc = "  \"description\": \"Runs sessions. Shown once at creation; only a hash is stored.\","]
#[doc = "  \"type\": \"string\","]
#[doc = "  \"pattern\": \"^aex_sk_[A-Za-z0-9]{40,64}$\""]
#[doc = "}"]
#[doc = r" ```"]
#[doc = r" </details>"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct ApiKeySecret(::std::string::String);
impl ::std::ops::Deref for ApiKeySecret {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<ApiKeySecret> for ::std::string::String {
    fn from(value: ApiKeySecret) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for ApiKeySecret {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| {
                ::regress::Regex::new("^aex_sk_[A-Za-z0-9]{40,64}$").unwrap()
            });
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^aex_sk_[A-Za-z0-9]{40,64}$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for ApiKeySecret {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<&::std::string::String> for ApiKeySecret {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ApiKeySecret {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for ApiKeySecret {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "Prepaid balance = credits minus rated usage, metered up to `metered_to`. May be negative: usage is rated after the fact; new sessions and messages are refused while it is not positive."]
#[doc = r""]
#[doc = r" <details><summary>JSON schema</summary>"]
#[doc = r""]
#[doc = r" ```json"]
#[doc = "{"]
#[doc = "  \"description\": \"Prepaid balance = credits minus rated usage, metered up to `metered_to`. May be negative: usage is rated after the fact; new sessions and messages are refused while it is not positive.\","]
#[doc = "  \"type\": \"object\","]
#[doc = "  \"required\": ["]
#[doc = "    \"metered_to\","]
#[doc = "    \"microusd\","]
#[doc = "    \"object\","]
#[doc = "    \"usd\""]
#[doc = "  ],"]
#[doc = "  \"properties\": {"]
#[doc = "    \"metered_to\": {"]
#[doc = "      \"$ref\": \"#/$defs/Timestamp\""]
#[doc = "    },"]
#[doc = "    \"microusd\": {"]
#[doc = "      \"$ref\": \"#/$defs/MicroUsd\""]
#[doc = "    },"]
#[doc = "    \"object\": {"]
#[doc = "      \"const\": \"balance\""]
#[doc = "    },"]
#[doc = "    \"usd\": {"]
#[doc = "      \"description\": \"Display form, whole cents, rounded toward zero.\","]
#[doc = "      \"type\": \"string\","]
#[doc = "      \"pattern\": \"^-?[0-9]+\\\\.[0-9]{2}$\""]
#[doc = "    }"]
#[doc = "  }"]
#[doc = "}"]
#[doc = r" ```"]
#[doc = r" </details>"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, PartialEq)]
pub struct Balance {
    pub metered_to: Timestamp,
    pub microusd: MicroUsd,
    pub object: ::serde_json::Value,
    #[doc = "Display form, whole cents, rounded toward zero."]
    pub usd: BalanceUsd,
}
impl Balance {
    pub fn builder() -> builder::Balance {
        Default::default()
    }
}
#[doc = "Display form, whole cents, rounded toward zero."]
#[doc = r""]
#[doc = r" <details><summary>JSON schema</summary>"]
#[doc = r""]
#[doc = r" ```json"]
#[doc = "{"]
#[doc = "  \"description\": \"Display form, whole cents, rounded toward zero.\","]
#[doc = "  \"type\": \"string\","]
#[doc = "  \"pattern\": \"^-?[0-9]+\\\\.[0-9]{2}$\""]
#[doc = "}"]
#[doc = r" ```"]
#[doc = r" </details>"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct BalanceUsd(::std::string::String);
impl ::std::ops::Deref for BalanceUsd {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<BalanceUsd> for ::std::string::String {
    fn from(value: BalanceUsd) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for BalanceUsd {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^-?[0-9]+\\.[0-9]{2}$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^-?[0-9]+\\.[0-9]{2}$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for BalanceUsd {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<&::std::string::String> for BalanceUsd {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for BalanceUsd {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for BalanceUsd {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`ControlError`"]
#[doc = r""]
#[doc = r" <details><summary>JSON schema</summary>"]
#[doc = r""]
#[doc = r" ```json"]
#[doc = "{"]
#[doc = "  \"type\": \"object\","]
#[doc = "  \"required\": ["]
#[doc = "    \"code\","]
#[doc = "    \"message\""]
#[doc = "  ],"]
#[doc = "  \"properties\": {"]
#[doc = "    \"code\": {"]
#[doc = "      \"$ref\": \"#/$defs/ControlErrorCode\""]
#[doc = "    },"]
#[doc = "    \"message\": {"]
#[doc = "      \"type\": \"string\""]
#[doc = "    },"]
#[doc = "    \"request_id\": {"]
#[doc = "      \"type\": \"string\""]
#[doc = "    }"]
#[doc = "  }"]
#[doc = "}"]
#[doc = r" ```"]
#[doc = r" </details>"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, PartialEq)]
pub struct ControlError {
    pub code: ControlErrorCode,
    pub message: ::std::string::String,
    #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
    pub request_id: ::std::option::Option<::std::string::String>,
}
impl ControlError {
    pub fn builder() -> builder::ControlError {
        Default::default()
    }
}
#[doc = "`ControlErrorCode`"]
#[doc = r""]
#[doc = r" <details><summary>JSON schema</summary>"]
#[doc = r""]
#[doc = r" ```json"]
#[doc = "{"]
#[doc = "  \"type\": \"string\","]
#[doc = "  \"enum\": ["]
#[doc = "    \"invalid_request\","]
#[doc = "    \"unauthorized\","]
#[doc = "    \"forbidden\","]
#[doc = "    \"not_found\","]
#[doc = "    \"conflict\","]
#[doc = "    \"insufficient_balance\","]
#[doc = "    \"rate_limited\","]
#[doc = "    \"payment_error\","]
#[doc = "    \"upstream_error\","]
#[doc = "    \"internal\""]
#[doc = "  ]"]
#[doc = "}"]
#[doc = r" ```"]
#[doc = r" </details>"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum ControlErrorCode {
    #[serde(rename = "invalid_request")]
    InvalidRequest,
    #[serde(rename = "unauthorized")]
    Unauthorized,
    #[serde(rename = "forbidden")]
    Forbidden,
    #[serde(rename = "not_found")]
    NotFound,
    #[serde(rename = "conflict")]
    Conflict,
    #[serde(rename = "insufficient_balance")]
    InsufficientBalance,
    #[serde(rename = "rate_limited")]
    RateLimited,
    #[serde(rename = "payment_error")]
    PaymentError,
    #[serde(rename = "upstream_error")]
    UpstreamError,
    #[serde(rename = "internal")]
    Internal,
}
impl ::std::fmt::Display for ControlErrorCode {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::InvalidRequest => f.write_str("invalid_request"),
            Self::Unauthorized => f.write_str("unauthorized"),
            Self::Forbidden => f.write_str("forbidden"),
            Self::NotFound => f.write_str("not_found"),
            Self::Conflict => f.write_str("conflict"),
            Self::InsufficientBalance => f.write_str("insufficient_balance"),
            Self::RateLimited => f.write_str("rate_limited"),
            Self::PaymentError => f.write_str("payment_error"),
            Self::UpstreamError => f.write_str("upstream_error"),
            Self::Internal => f.write_str("internal"),
        }
    }
}
impl ::std::str::FromStr for ControlErrorCode {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "invalid_request" => Ok(Self::InvalidRequest),
            "unauthorized" => Ok(Self::Unauthorized),
            "forbidden" => Ok(Self::Forbidden),
            "not_found" => Ok(Self::NotFound),
            "conflict" => Ok(Self::Conflict),
            "insufficient_balance" => Ok(Self::InsufficientBalance),
            "rate_limited" => Ok(Self::RateLimited),
            "payment_error" => Ok(Self::PaymentError),
            "upstream_error" => Ok(Self::UpstreamError),
            "internal" => Ok(Self::Internal),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for ControlErrorCode {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<&::std::string::String> for ControlErrorCode {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ControlErrorCode {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "Error envelope of the control plane's own endpoints. Proxied session/v1 endpoints keep the session ApiError envelope; the control plane injects only codes that ApiErrorCode already has (insufficient_balance, rate_limited, unauthorized, forbidden, not_found)."]
#[doc = r""]
#[doc = r" <details><summary>JSON schema</summary>"]
#[doc = r""]
#[doc = r" ```json"]
#[doc = "{"]
#[doc = "  \"description\": \"Error envelope of the control plane's own endpoints. Proxied session/v1 endpoints keep the session ApiError envelope; the control plane injects only codes that ApiErrorCode already has (insufficient_balance, rate_limited, unauthorized, forbidden, not_found).\","]
#[doc = "  \"type\": \"object\","]
#[doc = "  \"required\": ["]
#[doc = "    \"error\""]
#[doc = "  ],"]
#[doc = "  \"properties\": {"]
#[doc = "    \"error\": {"]
#[doc = "      \"$ref\": \"#/$defs/ControlError\""]
#[doc = "    }"]
#[doc = "  }"]
#[doc = "}"]
#[doc = r" ```"]
#[doc = r" </details>"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, PartialEq)]
pub struct ControlErrorResponse {
    pub error: ControlError,
}
impl ControlErrorResponse {
    pub fn builder() -> builder::ControlErrorResponse {
        Default::default()
    }
}
#[doc = "`CreateAccountRequest`"]
#[doc = r""]
#[doc = r" <details><summary>JSON schema</summary>"]
#[doc = r""]
#[doc = r" ```json"]
#[doc = "{"]
#[doc = "  \"type\": \"object\","]
#[doc = "  \"required\": ["]
#[doc = "    \"email\","]
#[doc = "    \"invite_token\""]
#[doc = "  ],"]
#[doc = "  \"properties\": {"]
#[doc = "    \"email\": {"]
#[doc = "      \"type\": \"string\","]
#[doc = "      \"maxLength\": 254,"]
#[doc = "      \"pattern\": \"^[^@\\\\s]+@[^@\\\\s]+\\\\.[^@\\\\s]+$\""]
#[doc = "    },"]
#[doc = "    \"invite_token\": {"]
#[doc = "      \"$ref\": \"#/$defs/InvitationToken\""]
#[doc = "    }"]
#[doc = "  },"]
#[doc = "  \"additionalProperties\": false"]
#[doc = "}"]
#[doc = r" ```"]
#[doc = r" </details>"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CreateAccountRequest {
    pub email: CreateAccountRequestEmail,
    pub invite_token: InvitationToken,
}
impl CreateAccountRequest {
    pub fn builder() -> builder::CreateAccountRequest {
        Default::default()
    }
}
#[doc = "`CreateAccountRequestEmail`"]
#[doc = r""]
#[doc = r" <details><summary>JSON schema</summary>"]
#[doc = r""]
#[doc = r" ```json"]
#[doc = "{"]
#[doc = "  \"type\": \"string\","]
#[doc = "  \"maxLength\": 254,"]
#[doc = "  \"pattern\": \"^[^@\\\\s]+@[^@\\\\s]+\\\\.[^@\\\\s]+$\""]
#[doc = "}"]
#[doc = r" ```"]
#[doc = r" </details>"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct CreateAccountRequestEmail(::std::string::String);
impl ::std::ops::Deref for CreateAccountRequestEmail {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<CreateAccountRequestEmail> for ::std::string::String {
    fn from(value: CreateAccountRequestEmail) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for CreateAccountRequestEmail {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        if value.chars().count() > 254usize {
            return Err("longer than 254 characters".into());
        }
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| {
                ::regress::Regex::new("^[^@\\s]+@[^@\\s]+\\.[^@\\s]+$").unwrap()
            });
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^[^@\\s]+@[^@\\s]+\\.[^@\\s]+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for CreateAccountRequestEmail {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<&::std::string::String> for CreateAccountRequestEmail {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for CreateAccountRequestEmail {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for CreateAccountRequestEmail {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`CreateApiKeyRequest`"]
#[doc = r""]
#[doc = r" <details><summary>JSON schema</summary>"]
#[doc = r""]
#[doc = r" ```json"]
#[doc = "{"]
#[doc = "  \"type\": \"object\","]
#[doc = "  \"required\": ["]
#[doc = "    \"name\""]
#[doc = "  ],"]
#[doc = "  \"properties\": {"]
#[doc = "    \"name\": {"]
#[doc = "      \"type\": \"string\","]
#[doc = "      \"maxLength\": 128,"]
#[doc = "      \"minLength\": 1"]
#[doc = "    }"]
#[doc = "  },"]
#[doc = "  \"additionalProperties\": false"]
#[doc = "}"]
#[doc = r" ```"]
#[doc = r" </details>"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CreateApiKeyRequest {
    pub name: CreateApiKeyRequestName,
}
impl CreateApiKeyRequest {
    pub fn builder() -> builder::CreateApiKeyRequest {
        Default::default()
    }
}
#[doc = "`CreateApiKeyRequestName`"]
#[doc = r""]
#[doc = r" <details><summary>JSON schema</summary>"]
#[doc = r""]
#[doc = r" ```json"]
#[doc = "{"]
#[doc = "  \"type\": \"string\","]
#[doc = "  \"maxLength\": 128,"]
#[doc = "  \"minLength\": 1"]
#[doc = "}"]
#[doc = r" ```"]
#[doc = r" </details>"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct CreateApiKeyRequestName(::std::string::String);
impl ::std::ops::Deref for CreateApiKeyRequestName {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<CreateApiKeyRequestName> for ::std::string::String {
    fn from(value: CreateApiKeyRequestName) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for CreateApiKeyRequestName {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        if value.chars().count() > 128usize {
            return Err("longer than 128 characters".into());
        }
        if value.chars().count() < 1usize {
            return Err("shorter than 1 characters".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for CreateApiKeyRequestName {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<&::std::string::String> for CreateApiKeyRequestName {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for CreateApiKeyRequestName {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for CreateApiKeyRequestName {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`CreateInvitationRequest`"]
#[doc = r""]
#[doc = r" <details><summary>JSON schema</summary>"]
#[doc = r""]
#[doc = r" ```json"]
#[doc = "{"]
#[doc = "  \"type\": \"object\","]
#[doc = "  \"required\": ["]
#[doc = "    \"email\""]
#[doc = "  ],"]
#[doc = "  \"properties\": {"]
#[doc = "    \"email\": {"]
#[doc = "      \"type\": \"string\","]
#[doc = "      \"maxLength\": 254,"]
#[doc = "      \"pattern\": \"^[^@\\\\s]+@[^@\\\\s]+\\\\.[^@\\\\s]+$\""]
#[doc = "    }"]
#[doc = "  },"]
#[doc = "  \"additionalProperties\": false"]
#[doc = "}"]
#[doc = r" ```"]
#[doc = r" </details>"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CreateInvitationRequest {
    pub email: CreateInvitationRequestEmail,
}
impl CreateInvitationRequest {
    pub fn builder() -> builder::CreateInvitationRequest {
        Default::default()
    }
}
#[doc = "`CreateInvitationRequestEmail`"]
#[doc = r""]
#[doc = r" <details><summary>JSON schema</summary>"]
#[doc = r""]
#[doc = r" ```json"]
#[doc = "{"]
#[doc = "  \"type\": \"string\","]
#[doc = "  \"maxLength\": 254,"]
#[doc = "  \"pattern\": \"^[^@\\\\s]+@[^@\\\\s]+\\\\.[^@\\\\s]+$\""]
#[doc = "}"]
#[doc = r" ```"]
#[doc = r" </details>"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct CreateInvitationRequestEmail(::std::string::String);
impl ::std::ops::Deref for CreateInvitationRequestEmail {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<CreateInvitationRequestEmail> for ::std::string::String {
    fn from(value: CreateInvitationRequestEmail) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for CreateInvitationRequestEmail {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        if value.chars().count() > 254usize {
            return Err("longer than 254 characters".into());
        }
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| {
                ::regress::Regex::new("^[^@\\s]+@[^@\\s]+\\.[^@\\s]+$").unwrap()
            });
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^[^@\\s]+@[^@\\s]+\\.[^@\\s]+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for CreateInvitationRequestEmail {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<&::std::string::String> for CreateInvitationRequestEmail {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for CreateInvitationRequestEmail {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for CreateInvitationRequestEmail {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`CreateRefundRequest`"]
#[doc = r""]
#[doc = r" <details><summary>JSON schema</summary>"]
#[doc = r""]
#[doc = r" ```json"]
#[doc = "{"]
#[doc = "  \"type\": \"object\","]
#[doc = "  \"required\": ["]
#[doc = "    \"amount_cents\","]
#[doc = "    \"topup_id\""]
#[doc = "  ],"]
#[doc = "  \"properties\": {"]
#[doc = "    \"amount_cents\": {"]
#[doc = "      \"description\": \"Whole cents of unused prepaid credit to return from this top-up.\","]
#[doc = "      \"type\": \"integer\","]
#[doc = "      \"maximum\": 100000.0,"]
#[doc = "      \"minimum\": 1.0"]
#[doc = "    },"]
#[doc = "    \"topup_id\": {"]
#[doc = "      \"$ref\": \"#/$defs/TopupId\""]
#[doc = "    }"]
#[doc = "  },"]
#[doc = "  \"additionalProperties\": false"]
#[doc = "}"]
#[doc = r" ```"]
#[doc = r" </details>"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CreateRefundRequest {
    #[doc = "Whole cents of unused prepaid credit to return from this top-up."]
    pub amount_cents: ::std::num::NonZeroU64,
    pub topup_id: TopupId,
}
impl CreateRefundRequest {
    pub fn builder() -> builder::CreateRefundRequest {
        Default::default()
    }
}
#[doc = "`CreateTopupRequest`"]
#[doc = r""]
#[doc = r" <details><summary>JSON schema</summary>"]
#[doc = r""]
#[doc = r" ```json"]
#[doc = "{"]
#[doc = "  \"type\": \"object\","]
#[doc = "  \"required\": ["]
#[doc = "    \"amount_cents\""]
#[doc = "  ],"]
#[doc = "  \"properties\": {"]
#[doc = "    \"amount_cents\": {"]
#[doc = "      \"description\": \"Whole cents. Founding Beta top-ups are $10.00 to $1,000.00.\","]
#[doc = "      \"type\": \"integer\","]
#[doc = "      \"maximum\": 100000.0,"]
#[doc = "      \"minimum\": 1000.0"]
#[doc = "    }"]
#[doc = "  },"]
#[doc = "  \"additionalProperties\": false"]
#[doc = "}"]
#[doc = r" ```"]
#[doc = r" </details>"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CreateTopupRequest {
    #[doc = "Whole cents. Founding Beta top-ups are $10.00 to $1,000.00."]
    pub amount_cents: i64,
}
impl CreateTopupRequest {
    pub fn builder() -> builder::CreateTopupRequest {
        Default::default()
    }
}
#[doc = "The invitation token appears here and never again. Creating another invitation rotates it."]
#[doc = r""]
#[doc = r" <details><summary>JSON schema</summary>"]
#[doc = r""]
#[doc = r" ```json"]
#[doc = "{"]
#[doc = "  \"description\": \"The invitation token appears here and never again. Creating another invitation rotates it.\","]
#[doc = "  \"type\": \"object\","]
#[doc = "  \"required\": ["]
#[doc = "    \"email\","]
#[doc = "    \"invite_token\","]
#[doc = "    \"invited_at\","]
#[doc = "    \"object\""]
#[doc = "  ],"]
#[doc = "  \"properties\": {"]
#[doc = "    \"email\": {"]
#[doc = "      \"type\": \"string\""]
#[doc = "    },"]
#[doc = "    \"invite_token\": {"]
#[doc = "      \"$ref\": \"#/$defs/InvitationToken\""]
#[doc = "    },"]
#[doc = "    \"invited_at\": {"]
#[doc = "      \"$ref\": \"#/$defs/Timestamp\""]
#[doc = "    },"]
#[doc = "    \"object\": {"]
#[doc = "      \"const\": \"invitation\""]
#[doc = "    }"]
#[doc = "  }"]
#[doc = "}"]
#[doc = r" ```"]
#[doc = r" </details>"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, PartialEq)]
pub struct InvitationCreated {
    pub email: ::std::string::String,
    pub invite_token: InvitationToken,
    pub invited_at: Timestamp,
    pub object: ::serde_json::Value,
}
impl InvitationCreated {
    pub fn builder() -> builder::InvitationCreated {
        Default::default()
    }
}
#[doc = "One-time Founding Beta invitation. Shown once to the operator; only a hash is stored."]
#[doc = r""]
#[doc = r" <details><summary>JSON schema</summary>"]
#[doc = r""]
#[doc = r" ```json"]
#[doc = "{"]
#[doc = "  \"description\": \"One-time Founding Beta invitation. Shown once to the operator; only a hash is stored.\","]
#[doc = "  \"type\": \"string\","]
#[doc = "  \"pattern\": \"^aex_iv_[A-Za-z0-9]{40,64}$\""]
#[doc = "}"]
#[doc = r" ```"]
#[doc = r" </details>"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct InvitationToken(::std::string::String);
impl ::std::ops::Deref for InvitationToken {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<InvitationToken> for ::std::string::String {
    fn from(value: InvitationToken) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for InvitationToken {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| {
                ::regress::Regex::new("^aex_iv_[A-Za-z0-9]{40,64}$").unwrap()
            });
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^aex_iv_[A-Za-z0-9]{40,64}$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for InvitationToken {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<&::std::string::String> for InvitationToken {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for InvitationToken {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for InvitationToken {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`JoinWaitlistRequest`"]
#[doc = r""]
#[doc = r" <details><summary>JSON schema</summary>"]
#[doc = r""]
#[doc = r" ```json"]
#[doc = "{"]
#[doc = "  \"type\": \"object\","]
#[doc = "  \"required\": ["]
#[doc = "    \"email\""]
#[doc = "  ],"]
#[doc = "  \"properties\": {"]
#[doc = "    \"email\": {"]
#[doc = "      \"type\": \"string\","]
#[doc = "      \"maxLength\": 254,"]
#[doc = "      \"pattern\": \"^[^@\\\\s]+@[^@\\\\s]+\\\\.[^@\\\\s]+$\""]
#[doc = "    }"]
#[doc = "  },"]
#[doc = "  \"additionalProperties\": false"]
#[doc = "}"]
#[doc = r" ```"]
#[doc = r" </details>"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct JoinWaitlistRequest {
    pub email: JoinWaitlistRequestEmail,
}
impl JoinWaitlistRequest {
    pub fn builder() -> builder::JoinWaitlistRequest {
        Default::default()
    }
}
#[doc = "`JoinWaitlistRequestEmail`"]
#[doc = r""]
#[doc = r" <details><summary>JSON schema</summary>"]
#[doc = r""]
#[doc = r" ```json"]
#[doc = "{"]
#[doc = "  \"type\": \"string\","]
#[doc = "  \"maxLength\": 254,"]
#[doc = "  \"pattern\": \"^[^@\\\\s]+@[^@\\\\s]+\\\\.[^@\\\\s]+$\""]
#[doc = "}"]
#[doc = r" ```"]
#[doc = r" </details>"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct JoinWaitlistRequestEmail(::std::string::String);
impl ::std::ops::Deref for JoinWaitlistRequestEmail {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<JoinWaitlistRequestEmail> for ::std::string::String {
    fn from(value: JoinWaitlistRequestEmail) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for JoinWaitlistRequestEmail {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        if value.chars().count() > 254usize {
            return Err("longer than 254 characters".into());
        }
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| {
                ::regress::Regex::new("^[^@\\s]+@[^@\\s]+\\.[^@\\s]+$").unwrap()
            });
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^[^@\\s]+@[^@\\s]+\\.[^@\\s]+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for JoinWaitlistRequestEmail {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<&::std::string::String> for JoinWaitlistRequestEmail {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for JoinWaitlistRequestEmail {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for JoinWaitlistRequestEmail {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`KeyId`"]
#[doc = r""]
#[doc = r" <details><summary>JSON schema</summary>"]
#[doc = r""]
#[doc = r" ```json"]
#[doc = "{"]
#[doc = "  \"type\": \"string\","]
#[doc = "  \"pattern\": \"^key_[A-Za-z0-9]{20,32}$\""]
#[doc = "}"]
#[doc = r" ```"]
#[doc = r" </details>"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct KeyId(::std::string::String);
impl ::std::ops::Deref for KeyId {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<KeyId> for ::std::string::String {
    fn from(value: KeyId) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for KeyId {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| {
                ::regress::Regex::new("^key_[A-Za-z0-9]{20,32}$").unwrap()
            });
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^key_[A-Za-z0-9]{20,32}$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for KeyId {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<&::std::string::String> for KeyId {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for KeyId {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for KeyId {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "Integer micro-USD; 1 USD = 1,000,000. Never a float."]
#[doc = r""]
#[doc = r" <details><summary>JSON schema</summary>"]
#[doc = r""]
#[doc = r" ```json"]
#[doc = "{"]
#[doc = "  \"description\": \"Integer micro-USD; 1 USD = 1,000,000. Never a float.\","]
#[doc = "  \"type\": \"integer\""]
#[doc = "}"]
#[doc = r" ```"]
#[doc = r" </details>"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, PartialEq)]
#[serde(transparent)]
pub struct MicroUsd(pub i64);
impl ::std::ops::Deref for MicroUsd {
    type Target = i64;
    fn deref(&self) -> &i64 {
        &self.0
    }
}
impl ::std::convert::From<MicroUsd> for i64 {
    fn from(value: MicroUsd) -> Self {
        value.0
    }
}
impl ::std::convert::From<i64> for MicroUsd {
    fn from(value: i64) -> Self {
        Self(value)
    }
}
impl ::std::str::FromStr for MicroUsd {
    type Err = <i64 as ::std::str::FromStr>::Err;
    fn from_str(value: &str) -> ::std::result::Result<Self, Self::Err> {
        Ok(Self(value.parse()?))
    }
}
impl ::std::convert::TryFrom<&str> for MicroUsd {
    type Error = <i64 as ::std::str::FromStr>::Err;
    fn try_from(value: &str) -> ::std::result::Result<Self, Self::Error> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<String> for MicroUsd {
    type Error = <i64 as ::std::str::FromStr>::Err;
    fn try_from(value: String) -> ::std::result::Result<Self, Self::Error> {
        value.parse()
    }
}
impl ::std::fmt::Display for MicroUsd {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        self.0.fmt(f)
    }
}
#[doc = "The two-rate card (ARCHITECTURE-v1 D4). Compute is billed per second while running on the shape's BASELINE (vCPU = memory/2; bursts are free); the pre-suspend idle window is absorbed. Suspended storage covers the bytes the substrate holds for a suspended hand; workspace storage covers synced workspace objects AND persisted artifacts. GB is decimal (1e9 bytes); a month is `month_hours` hours."]
#[doc = r""]
#[doc = r" <details><summary>JSON schema</summary>"]
#[doc = r""]
#[doc = r" ```json"]
#[doc = "{"]
#[doc = "  \"description\": \"The two-rate card (ARCHITECTURE-v1 D4). Compute is billed per second while running on the shape's BASELINE (vCPU = memory/2; bursts are free); the pre-suspend idle window is absorbed. Suspended storage covers the bytes the substrate holds for a suspended hand; workspace storage covers synced workspace objects AND persisted artifacts. GB is decimal (1e9 bytes); a month is `month_hours` hours.\","]
#[doc = "  \"type\": \"object\","]
#[doc = "  \"required\": ["]
#[doc = "    \"gb_hour_microusd\","]
#[doc = "    \"month_hours\","]
#[doc = "    \"object\","]
#[doc = "    \"suspended_gb_month_microusd\","]
#[doc = "    \"vcpu_hour_microusd\","]
#[doc = "    \"web_search_query_microusd\","]
#[doc = "    \"workspace_gb_month_microusd\""]
#[doc = "  ],"]
#[doc = "  \"properties\": {"]
#[doc = "    \"gb_hour_microusd\": {"]
#[doc = "      \"$ref\": \"#/$defs/MicroUsd\""]
#[doc = "    },"]
#[doc = "    \"month_hours\": {"]
#[doc = "      \"type\": \"integer\","]
#[doc = "      \"minimum\": 1.0"]
#[doc = "    },"]
#[doc = "    \"object\": {"]
#[doc = "      \"const\": \"rate_card\""]
#[doc = "    },"]
#[doc = "    \"suspended_gb_month_microusd\": {"]
#[doc = "      \"$ref\": \"#/$defs/MicroUsd\""]
#[doc = "    },"]
#[doc = "    \"vcpu_hour_microusd\": {"]
#[doc = "      \"$ref\": \"#/$defs/MicroUsd\""]
#[doc = "    },"]
#[doc = "    \"web_search_query_microusd\": {"]
#[doc = "      \"$ref\": \"#/$defs/MicroUsd\""]
#[doc = "    },"]
#[doc = "    \"workspace_gb_month_microusd\": {"]
#[doc = "      \"$ref\": \"#/$defs/MicroUsd\""]
#[doc = "    }"]
#[doc = "  }"]
#[doc = "}"]
#[doc = r" ```"]
#[doc = r" </details>"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, PartialEq)]
pub struct RateCard {
    pub gb_hour_microusd: MicroUsd,
    pub month_hours: ::std::num::NonZeroU64,
    pub object: ::serde_json::Value,
    pub suspended_gb_month_microusd: MicroUsd,
    pub vcpu_hour_microusd: MicroUsd,
    pub web_search_query_microusd: MicroUsd,
    pub workspace_gb_month_microusd: MicroUsd,
}
impl RateCard {
    pub fn builder() -> builder::RateCard {
        Default::default()
    }
}
#[doc = "An operator-initiated return of unused prepaid credit. Credit is reserved before the payment provider is called. Retrying the same Idempotency-Key returns the same refund."]
#[doc = r""]
#[doc = r" <details><summary>JSON schema</summary>"]
#[doc = r""]
#[doc = r" ```json"]
#[doc = "{"]
#[doc = "  \"description\": \"An operator-initiated return of unused prepaid credit. Credit is reserved before the payment provider is called. Retrying the same Idempotency-Key returns the same refund.\","]
#[doc = "  \"type\": \"object\","]
#[doc = "  \"required\": ["]
#[doc = "    \"amount_cents\","]
#[doc = "    \"created_at\","]
#[doc = "    \"id\","]
#[doc = "    \"object\","]
#[doc = "    \"status\","]
#[doc = "    \"topup_id\","]
#[doc = "    \"updated_at\""]
#[doc = "  ],"]
#[doc = "  \"properties\": {"]
#[doc = "    \"amount_cents\": {"]
#[doc = "      \"type\": \"integer\","]
#[doc = "      \"minimum\": 1.0"]
#[doc = "    },"]
#[doc = "    \"created_at\": {"]
#[doc = "      \"$ref\": \"#/$defs/Timestamp\""]
#[doc = "    },"]
#[doc = "    \"failure_reason\": {"]
#[doc = "      \"description\": \"Operator-facing payment-provider failure detail; present only when status is failed.\","]
#[doc = "      \"type\": \"string\""]
#[doc = "    },"]
#[doc = "    \"id\": {"]
#[doc = "      \"$ref\": \"#/$defs/RefundId\""]
#[doc = "    },"]
#[doc = "    \"object\": {"]
#[doc = "      \"const\": \"refund\""]
#[doc = "    },"]
#[doc = "    \"status\": {"]
#[doc = "      \"$ref\": \"#/$defs/RefundStatus\""]
#[doc = "    },"]
#[doc = "    \"topup_id\": {"]
#[doc = "      \"$ref\": \"#/$defs/TopupId\""]
#[doc = "    },"]
#[doc = "    \"updated_at\": {"]
#[doc = "      \"$ref\": \"#/$defs/Timestamp\""]
#[doc = "    }"]
#[doc = "  }"]
#[doc = "}"]
#[doc = r" ```"]
#[doc = r" </details>"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, PartialEq)]
pub struct Refund {
    pub amount_cents: ::std::num::NonZeroU64,
    pub created_at: Timestamp,
    #[doc = "Operator-facing payment-provider failure detail; present only when status is failed."]
    #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
    pub failure_reason: ::std::option::Option<::std::string::String>,
    pub id: RefundId,
    pub object: ::serde_json::Value,
    pub status: RefundStatus,
    pub topup_id: TopupId,
    pub updated_at: Timestamp,
}
impl Refund {
    pub fn builder() -> builder::Refund {
        Default::default()
    }
}
#[doc = "`RefundId`"]
#[doc = r""]
#[doc = r" <details><summary>JSON schema</summary>"]
#[doc = r""]
#[doc = r" ```json"]
#[doc = "{"]
#[doc = "  \"type\": \"string\","]
#[doc = "  \"pattern\": \"^rfd_[A-Za-z0-9]{20,32}$\""]
#[doc = "}"]
#[doc = r" ```"]
#[doc = r" </details>"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct RefundId(::std::string::String);
impl ::std::ops::Deref for RefundId {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<RefundId> for ::std::string::String {
    fn from(value: RefundId) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for RefundId {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| {
                ::regress::Regex::new("^rfd_[A-Za-z0-9]{20,32}$").unwrap()
            });
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^rfd_[A-Za-z0-9]{20,32}$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for RefundId {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<&::std::string::String> for RefundId {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for RefundId {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for RefundId {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`RefundStatus`"]
#[doc = r""]
#[doc = r" <details><summary>JSON schema</summary>"]
#[doc = r""]
#[doc = r" ```json"]
#[doc = "{"]
#[doc = "  \"type\": \"string\","]
#[doc = "  \"enum\": ["]
#[doc = "    \"pending\","]
#[doc = "    \"succeeded\","]
#[doc = "    \"failed\""]
#[doc = "  ]"]
#[doc = "}"]
#[doc = r" ```"]
#[doc = r" </details>"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum RefundStatus {
    #[serde(rename = "pending")]
    Pending,
    #[serde(rename = "succeeded")]
    Succeeded,
    #[serde(rename = "failed")]
    Failed,
}
impl ::std::fmt::Display for RefundStatus {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Pending => f.write_str("pending"),
            Self::Succeeded => f.write_str("succeeded"),
            Self::Failed => f.write_str("failed"),
        }
    }
}
impl ::std::str::FromStr for RefundStatus {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "pending" => Ok(Self::Pending),
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for RefundStatus {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<&::std::string::String> for RefundStatus {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for RefundStatus {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "One session's rated line. Compute time is the sum of turn intervals (turn.started to turn.completed/failed) folded from the session's event log — the journal is the billing record. Storage integrals are exact byte-seconds of the brain-reported meters, piecewise-constant between meter readings. Successful web_search tool results are counted from the same event log."]
#[doc = r""]
#[doc = r" <details><summary>JSON schema</summary>"]
#[doc = r""]
#[doc = r" ```json"]
#[doc = "{"]
#[doc = "  \"description\": \"One session's rated line. Compute time is the sum of turn intervals (turn.started to turn.completed/failed) folded from the session's event log — the journal is the billing record. Storage integrals are exact byte-seconds of the brain-reported meters, piecewise-constant between meter readings. Successful web_search tool results are counted from the same event log.\","]
#[doc = "  \"type\": \"object\","]
#[doc = "  \"required\": ["]
#[doc = "    \"artifact_byte_seconds\","]
#[doc = "    \"compute_microusd\","]
#[doc = "    \"metered_to\","]
#[doc = "    \"running_ms\","]
#[doc = "    \"session_id\","]
#[doc = "    \"shape\","]
#[doc = "    \"state\","]
#[doc = "    \"storage\","]
#[doc = "    \"storage_microusd\","]
#[doc = "    \"suspended_byte_seconds\","]
#[doc = "    \"total_microusd\","]
#[doc = "    \"web_search_microusd\","]
#[doc = "    \"web_search_queries\","]
#[doc = "    \"workspace_byte_seconds\""]
#[doc = "  ],"]
#[doc = "  \"properties\": {"]
#[doc = "    \"artifact_byte_seconds\": {"]
#[doc = "      \"type\": \"integer\","]
#[doc = "      \"minimum\": 0.0"]
#[doc = "    },"]
#[doc = "    \"compute_microusd\": {"]
#[doc = "      \"$ref\": \"#/$defs/MicroUsd\""]
#[doc = "    },"]
#[doc = "    \"metered_to\": {"]
#[doc = "      \"$ref\": \"#/$defs/Timestamp\""]
#[doc = "    },"]
#[doc = "    \"running_ms\": {"]
#[doc = "      \"type\": \"integer\","]
#[doc = "      \"minimum\": 0.0"]
#[doc = "    },"]
#[doc = "    \"session_id\": {"]
#[doc = "      \"type\": \"string\","]
#[doc = "      \"pattern\": \"^ses_[A-Za-z0-9]{20,32}$\""]
#[doc = "    },"]
#[doc = "    \"shape\": {"]
#[doc = "      \"description\": \"HandShape from session/v1 (1gb | 2gb | 4gb | 8gb).\","]
#[doc = "      \"type\": \"string\""]
#[doc = "    },"]
#[doc = "    \"state\": {"]
#[doc = "      \"description\": \"SessionState from session/v1 (active | idle | deleted | failed).\","]
#[doc = "      \"type\": \"string\""]
#[doc = "    },"]
#[doc = "    \"storage\": {"]
#[doc = "      \"$ref\": \"#/$defs/StorageMeters\""]
#[doc = "    },"]
#[doc = "    \"storage_microusd\": {"]
#[doc = "      \"$ref\": \"#/$defs/MicroUsd\""]
#[doc = "    },"]
#[doc = "    \"suspended_byte_seconds\": {"]
#[doc = "      \"type\": \"integer\","]
#[doc = "      \"minimum\": 0.0"]
#[doc = "    },"]
#[doc = "    \"total_microusd\": {"]
#[doc = "      \"$ref\": \"#/$defs/MicroUsd\""]
#[doc = "    },"]
#[doc = "    \"web_search_microusd\": {"]
#[doc = "      \"$ref\": \"#/$defs/MicroUsd\""]
#[doc = "    },"]
#[doc = "    \"web_search_queries\": {"]
#[doc = "      \"type\": \"integer\","]
#[doc = "      \"minimum\": 0.0"]
#[doc = "    },"]
#[doc = "    \"workspace_byte_seconds\": {"]
#[doc = "      \"type\": \"integer\","]
#[doc = "      \"minimum\": 0.0"]
#[doc = "    }"]
#[doc = "  }"]
#[doc = "}"]
#[doc = r" ```"]
#[doc = r" </details>"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, PartialEq)]
pub struct SessionUsage {
    pub artifact_byte_seconds: u64,
    pub compute_microusd: MicroUsd,
    pub metered_to: Timestamp,
    pub running_ms: u64,
    pub session_id: SessionUsageSessionId,
    #[doc = "HandShape from session/v1 (1gb | 2gb | 4gb | 8gb)."]
    pub shape: ::std::string::String,
    #[doc = "SessionState from session/v1 (active | idle | deleted | failed)."]
    pub state: ::std::string::String,
    pub storage: StorageMeters,
    pub storage_microusd: MicroUsd,
    pub suspended_byte_seconds: u64,
    pub total_microusd: MicroUsd,
    pub web_search_microusd: MicroUsd,
    pub web_search_queries: u64,
    pub workspace_byte_seconds: u64,
}
impl SessionUsage {
    pub fn builder() -> builder::SessionUsage {
        Default::default()
    }
}
#[doc = "`SessionUsageSessionId`"]
#[doc = r""]
#[doc = r" <details><summary>JSON schema</summary>"]
#[doc = r""]
#[doc = r" ```json"]
#[doc = "{"]
#[doc = "  \"type\": \"string\","]
#[doc = "  \"pattern\": \"^ses_[A-Za-z0-9]{20,32}$\""]
#[doc = "}"]
#[doc = r" ```"]
#[doc = r" </details>"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct SessionUsageSessionId(::std::string::String);
impl ::std::ops::Deref for SessionUsageSessionId {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<SessionUsageSessionId> for ::std::string::String {
    fn from(value: SessionUsageSessionId) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for SessionUsageSessionId {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| {
                ::regress::Regex::new("^ses_[A-Za-z0-9]{20,32}$").unwrap()
            });
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^ses_[A-Za-z0-9]{20,32}$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for SessionUsageSessionId {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<&::std::string::String> for SessionUsageSessionId {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for SessionUsageSessionId {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for SessionUsageSessionId {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "Current stored bytes, as last reported by the brain (session/v1 StorageInfo)."]
#[doc = r""]
#[doc = r" <details><summary>JSON schema</summary>"]
#[doc = r""]
#[doc = r" ```json"]
#[doc = "{"]
#[doc = "  \"description\": \"Current stored bytes, as last reported by the brain (session/v1 StorageInfo).\","]
#[doc = "  \"type\": \"object\","]
#[doc = "  \"required\": ["]
#[doc = "    \"artifact_bytes\","]
#[doc = "    \"suspended_bytes\","]
#[doc = "    \"workspace_bytes\""]
#[doc = "  ],"]
#[doc = "  \"properties\": {"]
#[doc = "    \"artifact_bytes\": {"]
#[doc = "      \"type\": \"integer\","]
#[doc = "      \"minimum\": 0.0"]
#[doc = "    },"]
#[doc = "    \"suspended_bytes\": {"]
#[doc = "      \"type\": \"integer\","]
#[doc = "      \"minimum\": 0.0"]
#[doc = "    },"]
#[doc = "    \"workspace_bytes\": {"]
#[doc = "      \"type\": \"integer\","]
#[doc = "      \"minimum\": 0.0"]
#[doc = "    }"]
#[doc = "  }"]
#[doc = "}"]
#[doc = r" ```"]
#[doc = r" </details>"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, PartialEq)]
pub struct StorageMeters {
    pub artifact_bytes: u64,
    pub suspended_bytes: u64,
    pub workspace_bytes: u64,
}
impl StorageMeters {
    pub fn builder() -> builder::StorageMeters {
        Default::default()
    }
}
#[doc = "RFC 3339, UTC."]
#[doc = r""]
#[doc = r" <details><summary>JSON schema</summary>"]
#[doc = r""]
#[doc = r" ```json"]
#[doc = "{"]
#[doc = "  \"description\": \"RFC 3339, UTC.\","]
#[doc = "  \"type\": \"string\","]
#[doc = "  \"format\": \"date-time\""]
#[doc = "}"]
#[doc = r" ```"]
#[doc = r" </details>"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, PartialEq)]
#[serde(transparent)]
pub struct Timestamp(pub ::chrono::DateTime<::chrono::offset::Utc>);
impl ::std::ops::Deref for Timestamp {
    type Target = ::chrono::DateTime<::chrono::offset::Utc>;
    fn deref(&self) -> &::chrono::DateTime<::chrono::offset::Utc> {
        &self.0
    }
}
impl ::std::convert::From<Timestamp> for ::chrono::DateTime<::chrono::offset::Utc> {
    fn from(value: Timestamp) -> Self {
        value.0
    }
}
impl ::std::convert::From<::chrono::DateTime<::chrono::offset::Utc>> for Timestamp {
    fn from(value: ::chrono::DateTime<::chrono::offset::Utc>) -> Self {
        Self(value)
    }
}
impl ::std::str::FromStr for Timestamp {
    type Err = <::chrono::DateTime<::chrono::offset::Utc> as ::std::str::FromStr>::Err;
    fn from_str(value: &str) -> ::std::result::Result<Self, Self::Err> {
        Ok(Self(value.parse()?))
    }
}
impl ::std::convert::TryFrom<&str> for Timestamp {
    type Error = <::chrono::DateTime<::chrono::offset::Utc> as ::std::str::FromStr>::Err;
    fn try_from(value: &str) -> ::std::result::Result<Self, Self::Error> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<String> for Timestamp {
    type Error = <::chrono::DateTime<::chrono::offset::Utc> as ::std::str::FromStr>::Err;
    fn try_from(value: String) -> ::std::result::Result<Self, Self::Error> {
        value.parse()
    }
}
impl ::std::fmt::Display for Timestamp {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        self.0.fmt(f)
    }
}
#[doc = "A prepaid credit purchase. `checkout_url` is where the customer pays (Stripe Checkout); present while pending. The balance is credited when the payment provider reports it paid — on webhook or on poll, idempotently."]
#[doc = r""]
#[doc = r" <details><summary>JSON schema</summary>"]
#[doc = r""]
#[doc = r" ```json"]
#[doc = "{"]
#[doc = "  \"description\": \"A prepaid credit purchase. `checkout_url` is where the customer pays (Stripe Checkout); present while pending. The balance is credited when the payment provider reports it paid — on webhook or on poll, idempotently.\","]
#[doc = "  \"type\": \"object\","]
#[doc = "  \"required\": ["]
#[doc = "    \"amount_cents\","]
#[doc = "    \"created_at\","]
#[doc = "    \"id\","]
#[doc = "    \"object\","]
#[doc = "    \"status\""]
#[doc = "  ],"]
#[doc = "  \"properties\": {"]
#[doc = "    \"amount_cents\": {"]
#[doc = "      \"type\": \"integer\","]
#[doc = "      \"minimum\": 1.0"]
#[doc = "    },"]
#[doc = "    \"checkout_url\": {"]
#[doc = "      \"type\": \"string\","]
#[doc = "      \"format\": \"uri\""]
#[doc = "    },"]
#[doc = "    \"created_at\": {"]
#[doc = "      \"$ref\": \"#/$defs/Timestamp\""]
#[doc = "    },"]
#[doc = "    \"id\": {"]
#[doc = "      \"$ref\": \"#/$defs/TopupId\""]
#[doc = "    },"]
#[doc = "    \"object\": {"]
#[doc = "      \"const\": \"topup\""]
#[doc = "    },"]
#[doc = "    \"paid_at\": {"]
#[doc = "      \"$ref\": \"#/$defs/Timestamp\""]
#[doc = "    },"]
#[doc = "    \"status\": {"]
#[doc = "      \"$ref\": \"#/$defs/TopupStatus\""]
#[doc = "    }"]
#[doc = "  }"]
#[doc = "}"]
#[doc = r" ```"]
#[doc = r" </details>"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, PartialEq)]
pub struct Topup {
    pub amount_cents: ::std::num::NonZeroU64,
    #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
    pub checkout_url: ::std::option::Option<::std::string::String>,
    pub created_at: Timestamp,
    pub id: TopupId,
    pub object: ::serde_json::Value,
    #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
    pub paid_at: ::std::option::Option<Timestamp>,
    pub status: TopupStatus,
}
impl Topup {
    pub fn builder() -> builder::Topup {
        Default::default()
    }
}
#[doc = "`TopupId`"]
#[doc = r""]
#[doc = r" <details><summary>JSON schema</summary>"]
#[doc = r""]
#[doc = r" ```json"]
#[doc = "{"]
#[doc = "  \"type\": \"string\","]
#[doc = "  \"pattern\": \"^top_[A-Za-z0-9]{20,32}$\""]
#[doc = "}"]
#[doc = r" ```"]
#[doc = r" </details>"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct TopupId(::std::string::String);
impl ::std::ops::Deref for TopupId {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<TopupId> for ::std::string::String {
    fn from(value: TopupId) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for TopupId {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| {
                ::regress::Regex::new("^top_[A-Za-z0-9]{20,32}$").unwrap()
            });
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^top_[A-Za-z0-9]{20,32}$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for TopupId {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<&::std::string::String> for TopupId {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for TopupId {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for TopupId {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`TopupList`"]
#[doc = r""]
#[doc = r" <details><summary>JSON schema</summary>"]
#[doc = r""]
#[doc = r" ```json"]
#[doc = "{"]
#[doc = "  \"type\": \"object\","]
#[doc = "  \"required\": ["]
#[doc = "    \"data\","]
#[doc = "    \"object\""]
#[doc = "  ],"]
#[doc = "  \"properties\": {"]
#[doc = "    \"data\": {"]
#[doc = "      \"type\": \"array\","]
#[doc = "      \"items\": {"]
#[doc = "        \"$ref\": \"#/$defs/Topup\""]
#[doc = "      }"]
#[doc = "    },"]
#[doc = "    \"object\": {"]
#[doc = "      \"const\": \"list\""]
#[doc = "    }"]
#[doc = "  }"]
#[doc = "}"]
#[doc = r" ```"]
#[doc = r" </details>"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, PartialEq)]
pub struct TopupList {
    pub data: ::std::vec::Vec<Topup>,
    pub object: ::serde_json::Value,
}
impl TopupList {
    pub fn builder() -> builder::TopupList {
        Default::default()
    }
}
#[doc = "`TopupStatus`"]
#[doc = r""]
#[doc = r" <details><summary>JSON schema</summary>"]
#[doc = r""]
#[doc = r" ```json"]
#[doc = "{"]
#[doc = "  \"type\": \"string\","]
#[doc = "  \"enum\": ["]
#[doc = "    \"pending\","]
#[doc = "    \"paid\","]
#[doc = "    \"expired\""]
#[doc = "  ]"]
#[doc = "}"]
#[doc = r" ```"]
#[doc = r" </details>"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum TopupStatus {
    #[serde(rename = "pending")]
    Pending,
    #[serde(rename = "paid")]
    Paid,
    #[serde(rename = "expired")]
    Expired,
}
impl ::std::fmt::Display for TopupStatus {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Pending => f.write_str("pending"),
            Self::Paid => f.write_str("paid"),
            Self::Expired => f.write_str("expired"),
        }
    }
}
impl ::std::str::FromStr for TopupStatus {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "pending" => Ok(Self::Pending),
            "paid" => Ok(Self::Paid),
            "expired" => Ok(Self::Expired),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for TopupStatus {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<&::std::string::String> for TopupStatus {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for TopupStatus {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "The bill: every session's rated line, the account balance after them, and the rate card they were rated on."]
#[doc = r""]
#[doc = r" <details><summary>JSON schema</summary>"]
#[doc = r""]
#[doc = r" ```json"]
#[doc = "{"]
#[doc = "  \"description\": \"The bill: every session's rated line, the account balance after them, and the rate card they were rated on.\","]
#[doc = "  \"type\": \"object\","]
#[doc = "  \"required\": ["]
#[doc = "    \"account_id\","]
#[doc = "    \"balance_microusd\","]
#[doc = "    \"metered_to\","]
#[doc = "    \"object\","]
#[doc = "    \"rates\","]
#[doc = "    \"sessions\","]
#[doc = "    \"total_microusd\""]
#[doc = "  ],"]
#[doc = "  \"properties\": {"]
#[doc = "    \"account_id\": {"]
#[doc = "      \"$ref\": \"#/$defs/AccountId\""]
#[doc = "    },"]
#[doc = "    \"balance_microusd\": {"]
#[doc = "      \"$ref\": \"#/$defs/MicroUsd\""]
#[doc = "    },"]
#[doc = "    \"metered_to\": {"]
#[doc = "      \"$ref\": \"#/$defs/Timestamp\""]
#[doc = "    },"]
#[doc = "    \"object\": {"]
#[doc = "      \"const\": \"usage\""]
#[doc = "    },"]
#[doc = "    \"rates\": {"]
#[doc = "      \"$ref\": \"#/$defs/RateCard\""]
#[doc = "    },"]
#[doc = "    \"sessions\": {"]
#[doc = "      \"type\": \"array\","]
#[doc = "      \"items\": {"]
#[doc = "        \"$ref\": \"#/$defs/SessionUsage\""]
#[doc = "      }"]
#[doc = "    },"]
#[doc = "    \"total_microusd\": {"]
#[doc = "      \"$ref\": \"#/$defs/MicroUsd\""]
#[doc = "    }"]
#[doc = "  }"]
#[doc = "}"]
#[doc = r" ```"]
#[doc = r" </details>"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, PartialEq)]
pub struct Usage {
    pub account_id: AccountId,
    pub balance_microusd: MicroUsd,
    pub metered_to: Timestamp,
    pub object: ::serde_json::Value,
    pub rates: RateCard,
    pub sessions: ::std::vec::Vec<SessionUsage>,
    pub total_microusd: MicroUsd,
}
impl Usage {
    pub fn builder() -> builder::Usage {
        Default::default()
    }
}
#[doc = "Operator view of one canonical Founding Beta waitlist record."]
#[doc = r""]
#[doc = r" <details><summary>JSON schema</summary>"]
#[doc = r""]
#[doc = r" ```json"]
#[doc = "{"]
#[doc = "  \"description\": \"Operator view of one canonical Founding Beta waitlist record.\","]
#[doc = "  \"type\": \"object\","]
#[doc = "  \"required\": ["]
#[doc = "    \"created_at\","]
#[doc = "    \"email\","]
#[doc = "    \"object\","]
#[doc = "    \"status\""]
#[doc = "  ],"]
#[doc = "  \"properties\": {"]
#[doc = "    \"created_at\": {"]
#[doc = "      \"$ref\": \"#/$defs/Timestamp\""]
#[doc = "    },"]
#[doc = "    \"email\": {"]
#[doc = "      \"type\": \"string\""]
#[doc = "    },"]
#[doc = "    \"invited_at\": {"]
#[doc = "      \"$ref\": \"#/$defs/Timestamp\""]
#[doc = "    },"]
#[doc = "    \"joined_at\": {"]
#[doc = "      \"$ref\": \"#/$defs/Timestamp\""]
#[doc = "    },"]
#[doc = "    \"object\": {"]
#[doc = "      \"const\": \"waitlist_entry\""]
#[doc = "    },"]
#[doc = "    \"status\": {"]
#[doc = "      \"$ref\": \"#/$defs/WaitlistStatus\""]
#[doc = "    }"]
#[doc = "  }"]
#[doc = "}"]
#[doc = r" ```"]
#[doc = r" </details>"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, PartialEq)]
pub struct WaitlistEntry {
    pub created_at: Timestamp,
    pub email: ::std::string::String,
    #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
    pub invited_at: ::std::option::Option<Timestamp>,
    #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
    pub joined_at: ::std::option::Option<Timestamp>,
    pub object: ::serde_json::Value,
    pub status: WaitlistStatus,
}
impl WaitlistEntry {
    pub fn builder() -> builder::WaitlistEntry {
        Default::default()
    }
}
#[doc = "`WaitlistEntryList`"]
#[doc = r""]
#[doc = r" <details><summary>JSON schema</summary>"]
#[doc = r""]
#[doc = r" ```json"]
#[doc = "{"]
#[doc = "  \"type\": \"object\","]
#[doc = "  \"required\": ["]
#[doc = "    \"data\","]
#[doc = "    \"object\""]
#[doc = "  ],"]
#[doc = "  \"properties\": {"]
#[doc = "    \"data\": {"]
#[doc = "      \"type\": \"array\","]
#[doc = "      \"items\": {"]
#[doc = "        \"$ref\": \"#/$defs/WaitlistEntry\""]
#[doc = "      }"]
#[doc = "    },"]
#[doc = "    \"object\": {"]
#[doc = "      \"const\": \"list\""]
#[doc = "    }"]
#[doc = "  }"]
#[doc = "}"]
#[doc = r" ```"]
#[doc = r" </details>"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, PartialEq)]
pub struct WaitlistEntryList {
    pub data: ::std::vec::Vec<WaitlistEntry>,
    pub object: ::serde_json::Value,
}
impl WaitlistEntryList {
    pub fn builder() -> builder::WaitlistEntryList {
        Default::default()
    }
}
#[doc = "`WaitlistStatus`"]
#[doc = r""]
#[doc = r" <details><summary>JSON schema</summary>"]
#[doc = r""]
#[doc = r" ```json"]
#[doc = "{"]
#[doc = "  \"type\": \"string\","]
#[doc = "  \"enum\": ["]
#[doc = "    \"waiting\","]
#[doc = "    \"invited\","]
#[doc = "    \"joined\""]
#[doc = "  ]"]
#[doc = "}"]
#[doc = r" ```"]
#[doc = r" </details>"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum WaitlistStatus {
    #[serde(rename = "waiting")]
    Waiting,
    #[serde(rename = "invited")]
    Invited,
    #[serde(rename = "joined")]
    Joined,
}
impl ::std::fmt::Display for WaitlistStatus {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Waiting => f.write_str("waiting"),
            Self::Invited => f.write_str("invited"),
            Self::Joined => f.write_str("joined"),
        }
    }
}
impl ::std::str::FromStr for WaitlistStatus {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "waiting" => Ok(Self::Waiting),
            "invited" => Ok(Self::Invited),
            "joined" => Ok(Self::Joined),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for WaitlistStatus {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<&::std::string::String> for WaitlistStatus {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for WaitlistStatus {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "A privacy-preserving acknowledgement. It does not reveal whether the email was already waiting, invited, or joined."]
#[doc = r""]
#[doc = r" <details><summary>JSON schema</summary>"]
#[doc = r""]
#[doc = r" ```json"]
#[doc = "{"]
#[doc = "  \"description\": \"A privacy-preserving acknowledgement. It does not reveal whether the email was already waiting, invited, or joined.\","]
#[doc = "  \"type\": \"object\","]
#[doc = "  \"required\": ["]
#[doc = "    \"email\","]
#[doc = "    \"object\","]
#[doc = "    \"received_at\","]
#[doc = "    \"status\""]
#[doc = "  ],"]
#[doc = "  \"properties\": {"]
#[doc = "    \"email\": {"]
#[doc = "      \"type\": \"string\""]
#[doc = "    },"]
#[doc = "    \"object\": {"]
#[doc = "      \"const\": \"waitlist_submission\""]
#[doc = "    },"]
#[doc = "    \"received_at\": {"]
#[doc = "      \"$ref\": \"#/$defs/Timestamp\""]
#[doc = "    },"]
#[doc = "    \"status\": {"]
#[doc = "      \"const\": \"received\""]
#[doc = "    }"]
#[doc = "  }"]
#[doc = "}"]
#[doc = r" ```"]
#[doc = r" </details>"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, PartialEq)]
pub struct WaitlistSubmission {
    pub email: ::std::string::String,
    pub object: ::serde_json::Value,
    pub received_at: Timestamp,
    pub status: ::serde_json::Value,
}
impl WaitlistSubmission {
    pub fn builder() -> builder::WaitlistSubmission {
        Default::default()
    }
}
#[doc = r" Types for composing complex structures."]
pub mod builder {
    #[derive(Clone, Debug)]
    pub struct Account {
        created_at: ::std::result::Result<super::Timestamp, ::std::string::String>,
        email: ::std::result::Result<::std::string::String, ::std::string::String>,
        id: ::std::result::Result<super::AccountId, ::std::string::String>,
        limits: ::std::result::Result<super::AccountLimits, ::std::string::String>,
        object: ::std::result::Result<::serde_json::Value, ::std::string::String>,
    }
    impl ::std::default::Default for Account {
        fn default() -> Self {
            Self {
                created_at: Err("no value supplied for created_at".to_string()),
                email: Err("no value supplied for email".to_string()),
                id: Err("no value supplied for id".to_string()),
                limits: Err("no value supplied for limits".to_string()),
                object: Err("no value supplied for object".to_string()),
            }
        }
    }
    impl Account {
        pub fn created_at<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::Timestamp>,
            T::Error: ::std::fmt::Display,
        {
            self.created_at = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for created_at: {e}"));
            self
        }
        pub fn email<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::string::String>,
            T::Error: ::std::fmt::Display,
        {
            self.email = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for email: {e}"));
            self
        }
        pub fn id<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::AccountId>,
            T::Error: ::std::fmt::Display,
        {
            self.id = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for id: {e}"));
            self
        }
        pub fn limits<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::AccountLimits>,
            T::Error: ::std::fmt::Display,
        {
            self.limits = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for limits: {e}"));
            self
        }
        pub fn object<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::serde_json::Value>,
            T::Error: ::std::fmt::Display,
        {
            self.object = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for object: {e}"));
            self
        }
    }
    impl ::std::convert::TryFrom<Account> for super::Account {
        type Error = super::error::ConversionError;
        fn try_from(value: Account) -> ::std::result::Result<Self, super::error::ConversionError> {
            Ok(Self {
                created_at: value.created_at?,
                email: value.email?,
                id: value.id?,
                limits: value.limits?,
                object: value.object?,
            })
        }
    }
    impl ::std::convert::From<super::Account> for Account {
        fn from(value: super::Account) -> Self {
            Self {
                created_at: Ok(value.created_at),
                email: Ok(value.email),
                id: Ok(value.id),
                limits: Ok(value.limits),
                object: Ok(value.object),
            }
        }
    }
    #[derive(Clone, Debug)]
    pub struct AccountCreated {
        account: ::std::result::Result<super::Account, ::std::string::String>,
        account_token: ::std::result::Result<super::AccountToken, ::std::string::String>,
    }
    impl ::std::default::Default for AccountCreated {
        fn default() -> Self {
            Self {
                account: Err("no value supplied for account".to_string()),
                account_token: Err("no value supplied for account_token".to_string()),
            }
        }
    }
    impl AccountCreated {
        pub fn account<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::Account>,
            T::Error: ::std::fmt::Display,
        {
            self.account = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for account: {e}"));
            self
        }
        pub fn account_token<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::AccountToken>,
            T::Error: ::std::fmt::Display,
        {
            self.account_token = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for account_token: {e}"));
            self
        }
    }
    impl ::std::convert::TryFrom<AccountCreated> for super::AccountCreated {
        type Error = super::error::ConversionError;
        fn try_from(
            value: AccountCreated,
        ) -> ::std::result::Result<Self, super::error::ConversionError> {
            Ok(Self {
                account: value.account?,
                account_token: value.account_token?,
            })
        }
    }
    impl ::std::convert::From<super::AccountCreated> for AccountCreated {
        fn from(value: super::AccountCreated) -> Self {
            Self {
                account: Ok(value.account),
                account_token: Ok(value.account_token),
            }
        }
    }
    #[derive(Clone, Debug)]
    pub struct AccountLimits {
        max_concurrent_sessions:
            ::std::result::Result<::std::num::NonZeroU64, ::std::string::String>,
        session_creates_per_hour:
            ::std::result::Result<::std::num::NonZeroU64, ::std::string::String>,
    }
    impl ::std::default::Default for AccountLimits {
        fn default() -> Self {
            Self {
                max_concurrent_sessions: Err(
                    "no value supplied for max_concurrent_sessions".to_string()
                ),
                session_creates_per_hour: Err(
                    "no value supplied for session_creates_per_hour".to_string()
                ),
            }
        }
    }
    impl AccountLimits {
        pub fn max_concurrent_sessions<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::num::NonZeroU64>,
            T::Error: ::std::fmt::Display,
        {
            self.max_concurrent_sessions = value.try_into().map_err(|e| {
                format!("error converting supplied value for max_concurrent_sessions: {e}")
            });
            self
        }
        pub fn session_creates_per_hour<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::num::NonZeroU64>,
            T::Error: ::std::fmt::Display,
        {
            self.session_creates_per_hour = value.try_into().map_err(|e| {
                format!("error converting supplied value for session_creates_per_hour: {e}")
            });
            self
        }
    }
    impl ::std::convert::TryFrom<AccountLimits> for super::AccountLimits {
        type Error = super::error::ConversionError;
        fn try_from(
            value: AccountLimits,
        ) -> ::std::result::Result<Self, super::error::ConversionError> {
            Ok(Self {
                max_concurrent_sessions: value.max_concurrent_sessions?,
                session_creates_per_hour: value.session_creates_per_hour?,
            })
        }
    }
    impl ::std::convert::From<super::AccountLimits> for AccountLimits {
        fn from(value: super::AccountLimits) -> Self {
            Self {
                max_concurrent_sessions: Ok(value.max_concurrent_sessions),
                session_creates_per_hour: Ok(value.session_creates_per_hour),
            }
        }
    }
    #[derive(Clone, Debug)]
    pub struct ApiKey {
        created_at: ::std::result::Result<super::Timestamp, ::std::string::String>,
        id: ::std::result::Result<super::KeyId, ::std::string::String>,
        last_used_at:
            ::std::result::Result<::std::option::Option<super::Timestamp>, ::std::string::String>,
        name: ::std::result::Result<super::ApiKeyName, ::std::string::String>,
        object: ::std::result::Result<::serde_json::Value, ::std::string::String>,
        prefix: ::std::result::Result<super::ApiKeyPrefix, ::std::string::String>,
        revoked_at:
            ::std::result::Result<::std::option::Option<super::Timestamp>, ::std::string::String>,
    }
    impl ::std::default::Default for ApiKey {
        fn default() -> Self {
            Self {
                created_at: Err("no value supplied for created_at".to_string()),
                id: Err("no value supplied for id".to_string()),
                last_used_at: Ok(Default::default()),
                name: Err("no value supplied for name".to_string()),
                object: Err("no value supplied for object".to_string()),
                prefix: Err("no value supplied for prefix".to_string()),
                revoked_at: Ok(Default::default()),
            }
        }
    }
    impl ApiKey {
        pub fn created_at<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::Timestamp>,
            T::Error: ::std::fmt::Display,
        {
            self.created_at = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for created_at: {e}"));
            self
        }
        pub fn id<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::KeyId>,
            T::Error: ::std::fmt::Display,
        {
            self.id = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for id: {e}"));
            self
        }
        pub fn last_used_at<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::option::Option<super::Timestamp>>,
            T::Error: ::std::fmt::Display,
        {
            self.last_used_at = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for last_used_at: {e}"));
            self
        }
        pub fn name<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::ApiKeyName>,
            T::Error: ::std::fmt::Display,
        {
            self.name = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for name: {e}"));
            self
        }
        pub fn object<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::serde_json::Value>,
            T::Error: ::std::fmt::Display,
        {
            self.object = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for object: {e}"));
            self
        }
        pub fn prefix<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::ApiKeyPrefix>,
            T::Error: ::std::fmt::Display,
        {
            self.prefix = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for prefix: {e}"));
            self
        }
        pub fn revoked_at<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::option::Option<super::Timestamp>>,
            T::Error: ::std::fmt::Display,
        {
            self.revoked_at = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for revoked_at: {e}"));
            self
        }
    }
    impl ::std::convert::TryFrom<ApiKey> for super::ApiKey {
        type Error = super::error::ConversionError;
        fn try_from(value: ApiKey) -> ::std::result::Result<Self, super::error::ConversionError> {
            Ok(Self {
                created_at: value.created_at?,
                id: value.id?,
                last_used_at: value.last_used_at?,
                name: value.name?,
                object: value.object?,
                prefix: value.prefix?,
                revoked_at: value.revoked_at?,
            })
        }
    }
    impl ::std::convert::From<super::ApiKey> for ApiKey {
        fn from(value: super::ApiKey) -> Self {
            Self {
                created_at: Ok(value.created_at),
                id: Ok(value.id),
                last_used_at: Ok(value.last_used_at),
                name: Ok(value.name),
                object: Ok(value.object),
                prefix: Ok(value.prefix),
                revoked_at: Ok(value.revoked_at),
            }
        }
    }
    #[derive(Clone, Debug)]
    pub struct ApiKeyCreated {
        key: ::std::result::Result<super::ApiKey, ::std::string::String>,
        secret: ::std::result::Result<super::ApiKeySecret, ::std::string::String>,
    }
    impl ::std::default::Default for ApiKeyCreated {
        fn default() -> Self {
            Self {
                key: Err("no value supplied for key".to_string()),
                secret: Err("no value supplied for secret".to_string()),
            }
        }
    }
    impl ApiKeyCreated {
        pub fn key<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::ApiKey>,
            T::Error: ::std::fmt::Display,
        {
            self.key = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for key: {e}"));
            self
        }
        pub fn secret<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::ApiKeySecret>,
            T::Error: ::std::fmt::Display,
        {
            self.secret = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for secret: {e}"));
            self
        }
    }
    impl ::std::convert::TryFrom<ApiKeyCreated> for super::ApiKeyCreated {
        type Error = super::error::ConversionError;
        fn try_from(
            value: ApiKeyCreated,
        ) -> ::std::result::Result<Self, super::error::ConversionError> {
            Ok(Self {
                key: value.key?,
                secret: value.secret?,
            })
        }
    }
    impl ::std::convert::From<super::ApiKeyCreated> for ApiKeyCreated {
        fn from(value: super::ApiKeyCreated) -> Self {
            Self {
                key: Ok(value.key),
                secret: Ok(value.secret),
            }
        }
    }
    #[derive(Clone, Debug)]
    pub struct ApiKeyList {
        data: ::std::result::Result<::std::vec::Vec<super::ApiKey>, ::std::string::String>,
        object: ::std::result::Result<::serde_json::Value, ::std::string::String>,
    }
    impl ::std::default::Default for ApiKeyList {
        fn default() -> Self {
            Self {
                data: Err("no value supplied for data".to_string()),
                object: Err("no value supplied for object".to_string()),
            }
        }
    }
    impl ApiKeyList {
        pub fn data<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::vec::Vec<super::ApiKey>>,
            T::Error: ::std::fmt::Display,
        {
            self.data = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for data: {e}"));
            self
        }
        pub fn object<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::serde_json::Value>,
            T::Error: ::std::fmt::Display,
        {
            self.object = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for object: {e}"));
            self
        }
    }
    impl ::std::convert::TryFrom<ApiKeyList> for super::ApiKeyList {
        type Error = super::error::ConversionError;
        fn try_from(
            value: ApiKeyList,
        ) -> ::std::result::Result<Self, super::error::ConversionError> {
            Ok(Self {
                data: value.data?,
                object: value.object?,
            })
        }
    }
    impl ::std::convert::From<super::ApiKeyList> for ApiKeyList {
        fn from(value: super::ApiKeyList) -> Self {
            Self {
                data: Ok(value.data),
                object: Ok(value.object),
            }
        }
    }
    #[derive(Clone, Debug)]
    pub struct Balance {
        metered_to: ::std::result::Result<super::Timestamp, ::std::string::String>,
        microusd: ::std::result::Result<super::MicroUsd, ::std::string::String>,
        object: ::std::result::Result<::serde_json::Value, ::std::string::String>,
        usd: ::std::result::Result<super::BalanceUsd, ::std::string::String>,
    }
    impl ::std::default::Default for Balance {
        fn default() -> Self {
            Self {
                metered_to: Err("no value supplied for metered_to".to_string()),
                microusd: Err("no value supplied for microusd".to_string()),
                object: Err("no value supplied for object".to_string()),
                usd: Err("no value supplied for usd".to_string()),
            }
        }
    }
    impl Balance {
        pub fn metered_to<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::Timestamp>,
            T::Error: ::std::fmt::Display,
        {
            self.metered_to = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for metered_to: {e}"));
            self
        }
        pub fn microusd<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::MicroUsd>,
            T::Error: ::std::fmt::Display,
        {
            self.microusd = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for microusd: {e}"));
            self
        }
        pub fn object<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::serde_json::Value>,
            T::Error: ::std::fmt::Display,
        {
            self.object = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for object: {e}"));
            self
        }
        pub fn usd<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::BalanceUsd>,
            T::Error: ::std::fmt::Display,
        {
            self.usd = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for usd: {e}"));
            self
        }
    }
    impl ::std::convert::TryFrom<Balance> for super::Balance {
        type Error = super::error::ConversionError;
        fn try_from(value: Balance) -> ::std::result::Result<Self, super::error::ConversionError> {
            Ok(Self {
                metered_to: value.metered_to?,
                microusd: value.microusd?,
                object: value.object?,
                usd: value.usd?,
            })
        }
    }
    impl ::std::convert::From<super::Balance> for Balance {
        fn from(value: super::Balance) -> Self {
            Self {
                metered_to: Ok(value.metered_to),
                microusd: Ok(value.microusd),
                object: Ok(value.object),
                usd: Ok(value.usd),
            }
        }
    }
    #[derive(Clone, Debug)]
    pub struct ControlError {
        code: ::std::result::Result<super::ControlErrorCode, ::std::string::String>,
        message: ::std::result::Result<::std::string::String, ::std::string::String>,
        request_id: ::std::result::Result<
            ::std::option::Option<::std::string::String>,
            ::std::string::String,
        >,
    }
    impl ::std::default::Default for ControlError {
        fn default() -> Self {
            Self {
                code: Err("no value supplied for code".to_string()),
                message: Err("no value supplied for message".to_string()),
                request_id: Ok(Default::default()),
            }
        }
    }
    impl ControlError {
        pub fn code<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::ControlErrorCode>,
            T::Error: ::std::fmt::Display,
        {
            self.code = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for code: {e}"));
            self
        }
        pub fn message<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::string::String>,
            T::Error: ::std::fmt::Display,
        {
            self.message = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for message: {e}"));
            self
        }
        pub fn request_id<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::option::Option<::std::string::String>>,
            T::Error: ::std::fmt::Display,
        {
            self.request_id = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for request_id: {e}"));
            self
        }
    }
    impl ::std::convert::TryFrom<ControlError> for super::ControlError {
        type Error = super::error::ConversionError;
        fn try_from(
            value: ControlError,
        ) -> ::std::result::Result<Self, super::error::ConversionError> {
            Ok(Self {
                code: value.code?,
                message: value.message?,
                request_id: value.request_id?,
            })
        }
    }
    impl ::std::convert::From<super::ControlError> for ControlError {
        fn from(value: super::ControlError) -> Self {
            Self {
                code: Ok(value.code),
                message: Ok(value.message),
                request_id: Ok(value.request_id),
            }
        }
    }
    #[derive(Clone, Debug)]
    pub struct ControlErrorResponse {
        error: ::std::result::Result<super::ControlError, ::std::string::String>,
    }
    impl ::std::default::Default for ControlErrorResponse {
        fn default() -> Self {
            Self {
                error: Err("no value supplied for error".to_string()),
            }
        }
    }
    impl ControlErrorResponse {
        pub fn error<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::ControlError>,
            T::Error: ::std::fmt::Display,
        {
            self.error = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for error: {e}"));
            self
        }
    }
    impl ::std::convert::TryFrom<ControlErrorResponse> for super::ControlErrorResponse {
        type Error = super::error::ConversionError;
        fn try_from(
            value: ControlErrorResponse,
        ) -> ::std::result::Result<Self, super::error::ConversionError> {
            Ok(Self {
                error: value.error?,
            })
        }
    }
    impl ::std::convert::From<super::ControlErrorResponse> for ControlErrorResponse {
        fn from(value: super::ControlErrorResponse) -> Self {
            Self {
                error: Ok(value.error),
            }
        }
    }
    #[derive(Clone, Debug)]
    pub struct CreateAccountRequest {
        email: ::std::result::Result<super::CreateAccountRequestEmail, ::std::string::String>,
        invite_token: ::std::result::Result<super::InvitationToken, ::std::string::String>,
    }
    impl ::std::default::Default for CreateAccountRequest {
        fn default() -> Self {
            Self {
                email: Err("no value supplied for email".to_string()),
                invite_token: Err("no value supplied for invite_token".to_string()),
            }
        }
    }
    impl CreateAccountRequest {
        pub fn email<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::CreateAccountRequestEmail>,
            T::Error: ::std::fmt::Display,
        {
            self.email = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for email: {e}"));
            self
        }
        pub fn invite_token<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::InvitationToken>,
            T::Error: ::std::fmt::Display,
        {
            self.invite_token = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for invite_token: {e}"));
            self
        }
    }
    impl ::std::convert::TryFrom<CreateAccountRequest> for super::CreateAccountRequest {
        type Error = super::error::ConversionError;
        fn try_from(
            value: CreateAccountRequest,
        ) -> ::std::result::Result<Self, super::error::ConversionError> {
            Ok(Self {
                email: value.email?,
                invite_token: value.invite_token?,
            })
        }
    }
    impl ::std::convert::From<super::CreateAccountRequest> for CreateAccountRequest {
        fn from(value: super::CreateAccountRequest) -> Self {
            Self {
                email: Ok(value.email),
                invite_token: Ok(value.invite_token),
            }
        }
    }
    #[derive(Clone, Debug)]
    pub struct CreateApiKeyRequest {
        name: ::std::result::Result<super::CreateApiKeyRequestName, ::std::string::String>,
    }
    impl ::std::default::Default for CreateApiKeyRequest {
        fn default() -> Self {
            Self {
                name: Err("no value supplied for name".to_string()),
            }
        }
    }
    impl CreateApiKeyRequest {
        pub fn name<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::CreateApiKeyRequestName>,
            T::Error: ::std::fmt::Display,
        {
            self.name = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for name: {e}"));
            self
        }
    }
    impl ::std::convert::TryFrom<CreateApiKeyRequest> for super::CreateApiKeyRequest {
        type Error = super::error::ConversionError;
        fn try_from(
            value: CreateApiKeyRequest,
        ) -> ::std::result::Result<Self, super::error::ConversionError> {
            Ok(Self { name: value.name? })
        }
    }
    impl ::std::convert::From<super::CreateApiKeyRequest> for CreateApiKeyRequest {
        fn from(value: super::CreateApiKeyRequest) -> Self {
            Self {
                name: Ok(value.name),
            }
        }
    }
    #[derive(Clone, Debug)]
    pub struct CreateInvitationRequest {
        email: ::std::result::Result<super::CreateInvitationRequestEmail, ::std::string::String>,
    }
    impl ::std::default::Default for CreateInvitationRequest {
        fn default() -> Self {
            Self {
                email: Err("no value supplied for email".to_string()),
            }
        }
    }
    impl CreateInvitationRequest {
        pub fn email<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::CreateInvitationRequestEmail>,
            T::Error: ::std::fmt::Display,
        {
            self.email = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for email: {e}"));
            self
        }
    }
    impl ::std::convert::TryFrom<CreateInvitationRequest> for super::CreateInvitationRequest {
        type Error = super::error::ConversionError;
        fn try_from(
            value: CreateInvitationRequest,
        ) -> ::std::result::Result<Self, super::error::ConversionError> {
            Ok(Self {
                email: value.email?,
            })
        }
    }
    impl ::std::convert::From<super::CreateInvitationRequest> for CreateInvitationRequest {
        fn from(value: super::CreateInvitationRequest) -> Self {
            Self {
                email: Ok(value.email),
            }
        }
    }
    #[derive(Clone, Debug)]
    pub struct CreateRefundRequest {
        amount_cents: ::std::result::Result<::std::num::NonZeroU64, ::std::string::String>,
        topup_id: ::std::result::Result<super::TopupId, ::std::string::String>,
    }
    impl ::std::default::Default for CreateRefundRequest {
        fn default() -> Self {
            Self {
                amount_cents: Err("no value supplied for amount_cents".to_string()),
                topup_id: Err("no value supplied for topup_id".to_string()),
            }
        }
    }
    impl CreateRefundRequest {
        pub fn amount_cents<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::num::NonZeroU64>,
            T::Error: ::std::fmt::Display,
        {
            self.amount_cents = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for amount_cents: {e}"));
            self
        }
        pub fn topup_id<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::TopupId>,
            T::Error: ::std::fmt::Display,
        {
            self.topup_id = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for topup_id: {e}"));
            self
        }
    }
    impl ::std::convert::TryFrom<CreateRefundRequest> for super::CreateRefundRequest {
        type Error = super::error::ConversionError;
        fn try_from(
            value: CreateRefundRequest,
        ) -> ::std::result::Result<Self, super::error::ConversionError> {
            Ok(Self {
                amount_cents: value.amount_cents?,
                topup_id: value.topup_id?,
            })
        }
    }
    impl ::std::convert::From<super::CreateRefundRequest> for CreateRefundRequest {
        fn from(value: super::CreateRefundRequest) -> Self {
            Self {
                amount_cents: Ok(value.amount_cents),
                topup_id: Ok(value.topup_id),
            }
        }
    }
    #[derive(Clone, Debug)]
    pub struct CreateTopupRequest {
        amount_cents: ::std::result::Result<i64, ::std::string::String>,
    }
    impl ::std::default::Default for CreateTopupRequest {
        fn default() -> Self {
            Self {
                amount_cents: Err("no value supplied for amount_cents".to_string()),
            }
        }
    }
    impl CreateTopupRequest {
        pub fn amount_cents<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<i64>,
            T::Error: ::std::fmt::Display,
        {
            self.amount_cents = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for amount_cents: {e}"));
            self
        }
    }
    impl ::std::convert::TryFrom<CreateTopupRequest> for super::CreateTopupRequest {
        type Error = super::error::ConversionError;
        fn try_from(
            value: CreateTopupRequest,
        ) -> ::std::result::Result<Self, super::error::ConversionError> {
            Ok(Self {
                amount_cents: value.amount_cents?,
            })
        }
    }
    impl ::std::convert::From<super::CreateTopupRequest> for CreateTopupRequest {
        fn from(value: super::CreateTopupRequest) -> Self {
            Self {
                amount_cents: Ok(value.amount_cents),
            }
        }
    }
    #[derive(Clone, Debug)]
    pub struct InvitationCreated {
        email: ::std::result::Result<::std::string::String, ::std::string::String>,
        invite_token: ::std::result::Result<super::InvitationToken, ::std::string::String>,
        invited_at: ::std::result::Result<super::Timestamp, ::std::string::String>,
        object: ::std::result::Result<::serde_json::Value, ::std::string::String>,
    }
    impl ::std::default::Default for InvitationCreated {
        fn default() -> Self {
            Self {
                email: Err("no value supplied for email".to_string()),
                invite_token: Err("no value supplied for invite_token".to_string()),
                invited_at: Err("no value supplied for invited_at".to_string()),
                object: Err("no value supplied for object".to_string()),
            }
        }
    }
    impl InvitationCreated {
        pub fn email<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::string::String>,
            T::Error: ::std::fmt::Display,
        {
            self.email = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for email: {e}"));
            self
        }
        pub fn invite_token<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::InvitationToken>,
            T::Error: ::std::fmt::Display,
        {
            self.invite_token = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for invite_token: {e}"));
            self
        }
        pub fn invited_at<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::Timestamp>,
            T::Error: ::std::fmt::Display,
        {
            self.invited_at = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for invited_at: {e}"));
            self
        }
        pub fn object<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::serde_json::Value>,
            T::Error: ::std::fmt::Display,
        {
            self.object = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for object: {e}"));
            self
        }
    }
    impl ::std::convert::TryFrom<InvitationCreated> for super::InvitationCreated {
        type Error = super::error::ConversionError;
        fn try_from(
            value: InvitationCreated,
        ) -> ::std::result::Result<Self, super::error::ConversionError> {
            Ok(Self {
                email: value.email?,
                invite_token: value.invite_token?,
                invited_at: value.invited_at?,
                object: value.object?,
            })
        }
    }
    impl ::std::convert::From<super::InvitationCreated> for InvitationCreated {
        fn from(value: super::InvitationCreated) -> Self {
            Self {
                email: Ok(value.email),
                invite_token: Ok(value.invite_token),
                invited_at: Ok(value.invited_at),
                object: Ok(value.object),
            }
        }
    }
    #[derive(Clone, Debug)]
    pub struct JoinWaitlistRequest {
        email: ::std::result::Result<super::JoinWaitlistRequestEmail, ::std::string::String>,
    }
    impl ::std::default::Default for JoinWaitlistRequest {
        fn default() -> Self {
            Self {
                email: Err("no value supplied for email".to_string()),
            }
        }
    }
    impl JoinWaitlistRequest {
        pub fn email<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::JoinWaitlistRequestEmail>,
            T::Error: ::std::fmt::Display,
        {
            self.email = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for email: {e}"));
            self
        }
    }
    impl ::std::convert::TryFrom<JoinWaitlistRequest> for super::JoinWaitlistRequest {
        type Error = super::error::ConversionError;
        fn try_from(
            value: JoinWaitlistRequest,
        ) -> ::std::result::Result<Self, super::error::ConversionError> {
            Ok(Self {
                email: value.email?,
            })
        }
    }
    impl ::std::convert::From<super::JoinWaitlistRequest> for JoinWaitlistRequest {
        fn from(value: super::JoinWaitlistRequest) -> Self {
            Self {
                email: Ok(value.email),
            }
        }
    }
    #[derive(Clone, Debug)]
    pub struct RateCard {
        gb_hour_microusd: ::std::result::Result<super::MicroUsd, ::std::string::String>,
        month_hours: ::std::result::Result<::std::num::NonZeroU64, ::std::string::String>,
        object: ::std::result::Result<::serde_json::Value, ::std::string::String>,
        suspended_gb_month_microusd: ::std::result::Result<super::MicroUsd, ::std::string::String>,
        vcpu_hour_microusd: ::std::result::Result<super::MicroUsd, ::std::string::String>,
        web_search_query_microusd: ::std::result::Result<super::MicroUsd, ::std::string::String>,
        workspace_gb_month_microusd: ::std::result::Result<super::MicroUsd, ::std::string::String>,
    }
    impl ::std::default::Default for RateCard {
        fn default() -> Self {
            Self {
                gb_hour_microusd: Err("no value supplied for gb_hour_microusd".to_string()),
                month_hours: Err("no value supplied for month_hours".to_string()),
                object: Err("no value supplied for object".to_string()),
                suspended_gb_month_microusd: Err(
                    "no value supplied for suspended_gb_month_microusd".to_string(),
                ),
                vcpu_hour_microusd: Err("no value supplied for vcpu_hour_microusd".to_string()),
                web_search_query_microusd: Err(
                    "no value supplied for web_search_query_microusd".to_string()
                ),
                workspace_gb_month_microusd: Err(
                    "no value supplied for workspace_gb_month_microusd".to_string(),
                ),
            }
        }
    }
    impl RateCard {
        pub fn gb_hour_microusd<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::MicroUsd>,
            T::Error: ::std::fmt::Display,
        {
            self.gb_hour_microusd = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for gb_hour_microusd: {e}"));
            self
        }
        pub fn month_hours<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::num::NonZeroU64>,
            T::Error: ::std::fmt::Display,
        {
            self.month_hours = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for month_hours: {e}"));
            self
        }
        pub fn object<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::serde_json::Value>,
            T::Error: ::std::fmt::Display,
        {
            self.object = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for object: {e}"));
            self
        }
        pub fn suspended_gb_month_microusd<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::MicroUsd>,
            T::Error: ::std::fmt::Display,
        {
            self.suspended_gb_month_microusd = value.try_into().map_err(|e| {
                format!("error converting supplied value for suspended_gb_month_microusd: {e}")
            });
            self
        }
        pub fn vcpu_hour_microusd<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::MicroUsd>,
            T::Error: ::std::fmt::Display,
        {
            self.vcpu_hour_microusd = value.try_into().map_err(|e| {
                format!("error converting supplied value for vcpu_hour_microusd: {e}")
            });
            self
        }
        pub fn web_search_query_microusd<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::MicroUsd>,
            T::Error: ::std::fmt::Display,
        {
            self.web_search_query_microusd = value.try_into().map_err(|e| {
                format!("error converting supplied value for web_search_query_microusd: {e}")
            });
            self
        }
        pub fn workspace_gb_month_microusd<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::MicroUsd>,
            T::Error: ::std::fmt::Display,
        {
            self.workspace_gb_month_microusd = value.try_into().map_err(|e| {
                format!("error converting supplied value for workspace_gb_month_microusd: {e}")
            });
            self
        }
    }
    impl ::std::convert::TryFrom<RateCard> for super::RateCard {
        type Error = super::error::ConversionError;
        fn try_from(value: RateCard) -> ::std::result::Result<Self, super::error::ConversionError> {
            Ok(Self {
                gb_hour_microusd: value.gb_hour_microusd?,
                month_hours: value.month_hours?,
                object: value.object?,
                suspended_gb_month_microusd: value.suspended_gb_month_microusd?,
                vcpu_hour_microusd: value.vcpu_hour_microusd?,
                web_search_query_microusd: value.web_search_query_microusd?,
                workspace_gb_month_microusd: value.workspace_gb_month_microusd?,
            })
        }
    }
    impl ::std::convert::From<super::RateCard> for RateCard {
        fn from(value: super::RateCard) -> Self {
            Self {
                gb_hour_microusd: Ok(value.gb_hour_microusd),
                month_hours: Ok(value.month_hours),
                object: Ok(value.object),
                suspended_gb_month_microusd: Ok(value.suspended_gb_month_microusd),
                vcpu_hour_microusd: Ok(value.vcpu_hour_microusd),
                web_search_query_microusd: Ok(value.web_search_query_microusd),
                workspace_gb_month_microusd: Ok(value.workspace_gb_month_microusd),
            }
        }
    }
    #[derive(Clone, Debug)]
    pub struct Refund {
        amount_cents: ::std::result::Result<::std::num::NonZeroU64, ::std::string::String>,
        created_at: ::std::result::Result<super::Timestamp, ::std::string::String>,
        failure_reason: ::std::result::Result<
            ::std::option::Option<::std::string::String>,
            ::std::string::String,
        >,
        id: ::std::result::Result<super::RefundId, ::std::string::String>,
        object: ::std::result::Result<::serde_json::Value, ::std::string::String>,
        status: ::std::result::Result<super::RefundStatus, ::std::string::String>,
        topup_id: ::std::result::Result<super::TopupId, ::std::string::String>,
        updated_at: ::std::result::Result<super::Timestamp, ::std::string::String>,
    }
    impl ::std::default::Default for Refund {
        fn default() -> Self {
            Self {
                amount_cents: Err("no value supplied for amount_cents".to_string()),
                created_at: Err("no value supplied for created_at".to_string()),
                failure_reason: Ok(Default::default()),
                id: Err("no value supplied for id".to_string()),
                object: Err("no value supplied for object".to_string()),
                status: Err("no value supplied for status".to_string()),
                topup_id: Err("no value supplied for topup_id".to_string()),
                updated_at: Err("no value supplied for updated_at".to_string()),
            }
        }
    }
    impl Refund {
        pub fn amount_cents<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::num::NonZeroU64>,
            T::Error: ::std::fmt::Display,
        {
            self.amount_cents = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for amount_cents: {e}"));
            self
        }
        pub fn created_at<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::Timestamp>,
            T::Error: ::std::fmt::Display,
        {
            self.created_at = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for created_at: {e}"));
            self
        }
        pub fn failure_reason<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::option::Option<::std::string::String>>,
            T::Error: ::std::fmt::Display,
        {
            self.failure_reason = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for failure_reason: {e}"));
            self
        }
        pub fn id<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::RefundId>,
            T::Error: ::std::fmt::Display,
        {
            self.id = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for id: {e}"));
            self
        }
        pub fn object<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::serde_json::Value>,
            T::Error: ::std::fmt::Display,
        {
            self.object = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for object: {e}"));
            self
        }
        pub fn status<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::RefundStatus>,
            T::Error: ::std::fmt::Display,
        {
            self.status = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for status: {e}"));
            self
        }
        pub fn topup_id<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::TopupId>,
            T::Error: ::std::fmt::Display,
        {
            self.topup_id = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for topup_id: {e}"));
            self
        }
        pub fn updated_at<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::Timestamp>,
            T::Error: ::std::fmt::Display,
        {
            self.updated_at = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for updated_at: {e}"));
            self
        }
    }
    impl ::std::convert::TryFrom<Refund> for super::Refund {
        type Error = super::error::ConversionError;
        fn try_from(value: Refund) -> ::std::result::Result<Self, super::error::ConversionError> {
            Ok(Self {
                amount_cents: value.amount_cents?,
                created_at: value.created_at?,
                failure_reason: value.failure_reason?,
                id: value.id?,
                object: value.object?,
                status: value.status?,
                topup_id: value.topup_id?,
                updated_at: value.updated_at?,
            })
        }
    }
    impl ::std::convert::From<super::Refund> for Refund {
        fn from(value: super::Refund) -> Self {
            Self {
                amount_cents: Ok(value.amount_cents),
                created_at: Ok(value.created_at),
                failure_reason: Ok(value.failure_reason),
                id: Ok(value.id),
                object: Ok(value.object),
                status: Ok(value.status),
                topup_id: Ok(value.topup_id),
                updated_at: Ok(value.updated_at),
            }
        }
    }
    #[derive(Clone, Debug)]
    pub struct SessionUsage {
        artifact_byte_seconds: ::std::result::Result<u64, ::std::string::String>,
        compute_microusd: ::std::result::Result<super::MicroUsd, ::std::string::String>,
        metered_to: ::std::result::Result<super::Timestamp, ::std::string::String>,
        running_ms: ::std::result::Result<u64, ::std::string::String>,
        session_id: ::std::result::Result<super::SessionUsageSessionId, ::std::string::String>,
        shape: ::std::result::Result<::std::string::String, ::std::string::String>,
        state: ::std::result::Result<::std::string::String, ::std::string::String>,
        storage: ::std::result::Result<super::StorageMeters, ::std::string::String>,
        storage_microusd: ::std::result::Result<super::MicroUsd, ::std::string::String>,
        suspended_byte_seconds: ::std::result::Result<u64, ::std::string::String>,
        total_microusd: ::std::result::Result<super::MicroUsd, ::std::string::String>,
        web_search_microusd: ::std::result::Result<super::MicroUsd, ::std::string::String>,
        web_search_queries: ::std::result::Result<u64, ::std::string::String>,
        workspace_byte_seconds: ::std::result::Result<u64, ::std::string::String>,
    }
    impl ::std::default::Default for SessionUsage {
        fn default() -> Self {
            Self {
                artifact_byte_seconds: Err(
                    "no value supplied for artifact_byte_seconds".to_string()
                ),
                compute_microusd: Err("no value supplied for compute_microusd".to_string()),
                metered_to: Err("no value supplied for metered_to".to_string()),
                running_ms: Err("no value supplied for running_ms".to_string()),
                session_id: Err("no value supplied for session_id".to_string()),
                shape: Err("no value supplied for shape".to_string()),
                state: Err("no value supplied for state".to_string()),
                storage: Err("no value supplied for storage".to_string()),
                storage_microusd: Err("no value supplied for storage_microusd".to_string()),
                suspended_byte_seconds: Err(
                    "no value supplied for suspended_byte_seconds".to_string()
                ),
                total_microusd: Err("no value supplied for total_microusd".to_string()),
                web_search_microusd: Err("no value supplied for web_search_microusd".to_string()),
                web_search_queries: Err("no value supplied for web_search_queries".to_string()),
                workspace_byte_seconds: Err(
                    "no value supplied for workspace_byte_seconds".to_string()
                ),
            }
        }
    }
    impl SessionUsage {
        pub fn artifact_byte_seconds<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<u64>,
            T::Error: ::std::fmt::Display,
        {
            self.artifact_byte_seconds = value.try_into().map_err(|e| {
                format!("error converting supplied value for artifact_byte_seconds: {e}")
            });
            self
        }
        pub fn compute_microusd<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::MicroUsd>,
            T::Error: ::std::fmt::Display,
        {
            self.compute_microusd = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for compute_microusd: {e}"));
            self
        }
        pub fn metered_to<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::Timestamp>,
            T::Error: ::std::fmt::Display,
        {
            self.metered_to = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for metered_to: {e}"));
            self
        }
        pub fn running_ms<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<u64>,
            T::Error: ::std::fmt::Display,
        {
            self.running_ms = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for running_ms: {e}"));
            self
        }
        pub fn session_id<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::SessionUsageSessionId>,
            T::Error: ::std::fmt::Display,
        {
            self.session_id = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for session_id: {e}"));
            self
        }
        pub fn shape<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::string::String>,
            T::Error: ::std::fmt::Display,
        {
            self.shape = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for shape: {e}"));
            self
        }
        pub fn state<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::string::String>,
            T::Error: ::std::fmt::Display,
        {
            self.state = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for state: {e}"));
            self
        }
        pub fn storage<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::StorageMeters>,
            T::Error: ::std::fmt::Display,
        {
            self.storage = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for storage: {e}"));
            self
        }
        pub fn storage_microusd<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::MicroUsd>,
            T::Error: ::std::fmt::Display,
        {
            self.storage_microusd = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for storage_microusd: {e}"));
            self
        }
        pub fn suspended_byte_seconds<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<u64>,
            T::Error: ::std::fmt::Display,
        {
            self.suspended_byte_seconds = value.try_into().map_err(|e| {
                format!("error converting supplied value for suspended_byte_seconds: {e}")
            });
            self
        }
        pub fn total_microusd<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::MicroUsd>,
            T::Error: ::std::fmt::Display,
        {
            self.total_microusd = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for total_microusd: {e}"));
            self
        }
        pub fn web_search_microusd<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::MicroUsd>,
            T::Error: ::std::fmt::Display,
        {
            self.web_search_microusd = value.try_into().map_err(|e| {
                format!("error converting supplied value for web_search_microusd: {e}")
            });
            self
        }
        pub fn web_search_queries<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<u64>,
            T::Error: ::std::fmt::Display,
        {
            self.web_search_queries = value.try_into().map_err(|e| {
                format!("error converting supplied value for web_search_queries: {e}")
            });
            self
        }
        pub fn workspace_byte_seconds<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<u64>,
            T::Error: ::std::fmt::Display,
        {
            self.workspace_byte_seconds = value.try_into().map_err(|e| {
                format!("error converting supplied value for workspace_byte_seconds: {e}")
            });
            self
        }
    }
    impl ::std::convert::TryFrom<SessionUsage> for super::SessionUsage {
        type Error = super::error::ConversionError;
        fn try_from(
            value: SessionUsage,
        ) -> ::std::result::Result<Self, super::error::ConversionError> {
            Ok(Self {
                artifact_byte_seconds: value.artifact_byte_seconds?,
                compute_microusd: value.compute_microusd?,
                metered_to: value.metered_to?,
                running_ms: value.running_ms?,
                session_id: value.session_id?,
                shape: value.shape?,
                state: value.state?,
                storage: value.storage?,
                storage_microusd: value.storage_microusd?,
                suspended_byte_seconds: value.suspended_byte_seconds?,
                total_microusd: value.total_microusd?,
                web_search_microusd: value.web_search_microusd?,
                web_search_queries: value.web_search_queries?,
                workspace_byte_seconds: value.workspace_byte_seconds?,
            })
        }
    }
    impl ::std::convert::From<super::SessionUsage> for SessionUsage {
        fn from(value: super::SessionUsage) -> Self {
            Self {
                artifact_byte_seconds: Ok(value.artifact_byte_seconds),
                compute_microusd: Ok(value.compute_microusd),
                metered_to: Ok(value.metered_to),
                running_ms: Ok(value.running_ms),
                session_id: Ok(value.session_id),
                shape: Ok(value.shape),
                state: Ok(value.state),
                storage: Ok(value.storage),
                storage_microusd: Ok(value.storage_microusd),
                suspended_byte_seconds: Ok(value.suspended_byte_seconds),
                total_microusd: Ok(value.total_microusd),
                web_search_microusd: Ok(value.web_search_microusd),
                web_search_queries: Ok(value.web_search_queries),
                workspace_byte_seconds: Ok(value.workspace_byte_seconds),
            }
        }
    }
    #[derive(Clone, Debug)]
    pub struct StorageMeters {
        artifact_bytes: ::std::result::Result<u64, ::std::string::String>,
        suspended_bytes: ::std::result::Result<u64, ::std::string::String>,
        workspace_bytes: ::std::result::Result<u64, ::std::string::String>,
    }
    impl ::std::default::Default for StorageMeters {
        fn default() -> Self {
            Self {
                artifact_bytes: Err("no value supplied for artifact_bytes".to_string()),
                suspended_bytes: Err("no value supplied for suspended_bytes".to_string()),
                workspace_bytes: Err("no value supplied for workspace_bytes".to_string()),
            }
        }
    }
    impl StorageMeters {
        pub fn artifact_bytes<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<u64>,
            T::Error: ::std::fmt::Display,
        {
            self.artifact_bytes = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for artifact_bytes: {e}"));
            self
        }
        pub fn suspended_bytes<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<u64>,
            T::Error: ::std::fmt::Display,
        {
            self.suspended_bytes = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for suspended_bytes: {e}"));
            self
        }
        pub fn workspace_bytes<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<u64>,
            T::Error: ::std::fmt::Display,
        {
            self.workspace_bytes = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for workspace_bytes: {e}"));
            self
        }
    }
    impl ::std::convert::TryFrom<StorageMeters> for super::StorageMeters {
        type Error = super::error::ConversionError;
        fn try_from(
            value: StorageMeters,
        ) -> ::std::result::Result<Self, super::error::ConversionError> {
            Ok(Self {
                artifact_bytes: value.artifact_bytes?,
                suspended_bytes: value.suspended_bytes?,
                workspace_bytes: value.workspace_bytes?,
            })
        }
    }
    impl ::std::convert::From<super::StorageMeters> for StorageMeters {
        fn from(value: super::StorageMeters) -> Self {
            Self {
                artifact_bytes: Ok(value.artifact_bytes),
                suspended_bytes: Ok(value.suspended_bytes),
                workspace_bytes: Ok(value.workspace_bytes),
            }
        }
    }
    #[derive(Clone, Debug)]
    pub struct Topup {
        amount_cents: ::std::result::Result<::std::num::NonZeroU64, ::std::string::String>,
        checkout_url: ::std::result::Result<
            ::std::option::Option<::std::string::String>,
            ::std::string::String,
        >,
        created_at: ::std::result::Result<super::Timestamp, ::std::string::String>,
        id: ::std::result::Result<super::TopupId, ::std::string::String>,
        object: ::std::result::Result<::serde_json::Value, ::std::string::String>,
        paid_at:
            ::std::result::Result<::std::option::Option<super::Timestamp>, ::std::string::String>,
        status: ::std::result::Result<super::TopupStatus, ::std::string::String>,
    }
    impl ::std::default::Default for Topup {
        fn default() -> Self {
            Self {
                amount_cents: Err("no value supplied for amount_cents".to_string()),
                checkout_url: Ok(Default::default()),
                created_at: Err("no value supplied for created_at".to_string()),
                id: Err("no value supplied for id".to_string()),
                object: Err("no value supplied for object".to_string()),
                paid_at: Ok(Default::default()),
                status: Err("no value supplied for status".to_string()),
            }
        }
    }
    impl Topup {
        pub fn amount_cents<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::num::NonZeroU64>,
            T::Error: ::std::fmt::Display,
        {
            self.amount_cents = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for amount_cents: {e}"));
            self
        }
        pub fn checkout_url<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::option::Option<::std::string::String>>,
            T::Error: ::std::fmt::Display,
        {
            self.checkout_url = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for checkout_url: {e}"));
            self
        }
        pub fn created_at<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::Timestamp>,
            T::Error: ::std::fmt::Display,
        {
            self.created_at = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for created_at: {e}"));
            self
        }
        pub fn id<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::TopupId>,
            T::Error: ::std::fmt::Display,
        {
            self.id = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for id: {e}"));
            self
        }
        pub fn object<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::serde_json::Value>,
            T::Error: ::std::fmt::Display,
        {
            self.object = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for object: {e}"));
            self
        }
        pub fn paid_at<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::option::Option<super::Timestamp>>,
            T::Error: ::std::fmt::Display,
        {
            self.paid_at = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for paid_at: {e}"));
            self
        }
        pub fn status<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::TopupStatus>,
            T::Error: ::std::fmt::Display,
        {
            self.status = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for status: {e}"));
            self
        }
    }
    impl ::std::convert::TryFrom<Topup> for super::Topup {
        type Error = super::error::ConversionError;
        fn try_from(value: Topup) -> ::std::result::Result<Self, super::error::ConversionError> {
            Ok(Self {
                amount_cents: value.amount_cents?,
                checkout_url: value.checkout_url?,
                created_at: value.created_at?,
                id: value.id?,
                object: value.object?,
                paid_at: value.paid_at?,
                status: value.status?,
            })
        }
    }
    impl ::std::convert::From<super::Topup> for Topup {
        fn from(value: super::Topup) -> Self {
            Self {
                amount_cents: Ok(value.amount_cents),
                checkout_url: Ok(value.checkout_url),
                created_at: Ok(value.created_at),
                id: Ok(value.id),
                object: Ok(value.object),
                paid_at: Ok(value.paid_at),
                status: Ok(value.status),
            }
        }
    }
    #[derive(Clone, Debug)]
    pub struct TopupList {
        data: ::std::result::Result<::std::vec::Vec<super::Topup>, ::std::string::String>,
        object: ::std::result::Result<::serde_json::Value, ::std::string::String>,
    }
    impl ::std::default::Default for TopupList {
        fn default() -> Self {
            Self {
                data: Err("no value supplied for data".to_string()),
                object: Err("no value supplied for object".to_string()),
            }
        }
    }
    impl TopupList {
        pub fn data<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::vec::Vec<super::Topup>>,
            T::Error: ::std::fmt::Display,
        {
            self.data = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for data: {e}"));
            self
        }
        pub fn object<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::serde_json::Value>,
            T::Error: ::std::fmt::Display,
        {
            self.object = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for object: {e}"));
            self
        }
    }
    impl ::std::convert::TryFrom<TopupList> for super::TopupList {
        type Error = super::error::ConversionError;
        fn try_from(
            value: TopupList,
        ) -> ::std::result::Result<Self, super::error::ConversionError> {
            Ok(Self {
                data: value.data?,
                object: value.object?,
            })
        }
    }
    impl ::std::convert::From<super::TopupList> for TopupList {
        fn from(value: super::TopupList) -> Self {
            Self {
                data: Ok(value.data),
                object: Ok(value.object),
            }
        }
    }
    #[derive(Clone, Debug)]
    pub struct Usage {
        account_id: ::std::result::Result<super::AccountId, ::std::string::String>,
        balance_microusd: ::std::result::Result<super::MicroUsd, ::std::string::String>,
        metered_to: ::std::result::Result<super::Timestamp, ::std::string::String>,
        object: ::std::result::Result<::serde_json::Value, ::std::string::String>,
        rates: ::std::result::Result<super::RateCard, ::std::string::String>,
        sessions:
            ::std::result::Result<::std::vec::Vec<super::SessionUsage>, ::std::string::String>,
        total_microusd: ::std::result::Result<super::MicroUsd, ::std::string::String>,
    }
    impl ::std::default::Default for Usage {
        fn default() -> Self {
            Self {
                account_id: Err("no value supplied for account_id".to_string()),
                balance_microusd: Err("no value supplied for balance_microusd".to_string()),
                metered_to: Err("no value supplied for metered_to".to_string()),
                object: Err("no value supplied for object".to_string()),
                rates: Err("no value supplied for rates".to_string()),
                sessions: Err("no value supplied for sessions".to_string()),
                total_microusd: Err("no value supplied for total_microusd".to_string()),
            }
        }
    }
    impl Usage {
        pub fn account_id<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::AccountId>,
            T::Error: ::std::fmt::Display,
        {
            self.account_id = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for account_id: {e}"));
            self
        }
        pub fn balance_microusd<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::MicroUsd>,
            T::Error: ::std::fmt::Display,
        {
            self.balance_microusd = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for balance_microusd: {e}"));
            self
        }
        pub fn metered_to<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::Timestamp>,
            T::Error: ::std::fmt::Display,
        {
            self.metered_to = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for metered_to: {e}"));
            self
        }
        pub fn object<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::serde_json::Value>,
            T::Error: ::std::fmt::Display,
        {
            self.object = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for object: {e}"));
            self
        }
        pub fn rates<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::RateCard>,
            T::Error: ::std::fmt::Display,
        {
            self.rates = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for rates: {e}"));
            self
        }
        pub fn sessions<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::vec::Vec<super::SessionUsage>>,
            T::Error: ::std::fmt::Display,
        {
            self.sessions = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for sessions: {e}"));
            self
        }
        pub fn total_microusd<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::MicroUsd>,
            T::Error: ::std::fmt::Display,
        {
            self.total_microusd = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for total_microusd: {e}"));
            self
        }
    }
    impl ::std::convert::TryFrom<Usage> for super::Usage {
        type Error = super::error::ConversionError;
        fn try_from(value: Usage) -> ::std::result::Result<Self, super::error::ConversionError> {
            Ok(Self {
                account_id: value.account_id?,
                balance_microusd: value.balance_microusd?,
                metered_to: value.metered_to?,
                object: value.object?,
                rates: value.rates?,
                sessions: value.sessions?,
                total_microusd: value.total_microusd?,
            })
        }
    }
    impl ::std::convert::From<super::Usage> for Usage {
        fn from(value: super::Usage) -> Self {
            Self {
                account_id: Ok(value.account_id),
                balance_microusd: Ok(value.balance_microusd),
                metered_to: Ok(value.metered_to),
                object: Ok(value.object),
                rates: Ok(value.rates),
                sessions: Ok(value.sessions),
                total_microusd: Ok(value.total_microusd),
            }
        }
    }
    #[derive(Clone, Debug)]
    pub struct WaitlistEntry {
        created_at: ::std::result::Result<super::Timestamp, ::std::string::String>,
        email: ::std::result::Result<::std::string::String, ::std::string::String>,
        invited_at:
            ::std::result::Result<::std::option::Option<super::Timestamp>, ::std::string::String>,
        joined_at:
            ::std::result::Result<::std::option::Option<super::Timestamp>, ::std::string::String>,
        object: ::std::result::Result<::serde_json::Value, ::std::string::String>,
        status: ::std::result::Result<super::WaitlistStatus, ::std::string::String>,
    }
    impl ::std::default::Default for WaitlistEntry {
        fn default() -> Self {
            Self {
                created_at: Err("no value supplied for created_at".to_string()),
                email: Err("no value supplied for email".to_string()),
                invited_at: Ok(Default::default()),
                joined_at: Ok(Default::default()),
                object: Err("no value supplied for object".to_string()),
                status: Err("no value supplied for status".to_string()),
            }
        }
    }
    impl WaitlistEntry {
        pub fn created_at<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::Timestamp>,
            T::Error: ::std::fmt::Display,
        {
            self.created_at = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for created_at: {e}"));
            self
        }
        pub fn email<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::string::String>,
            T::Error: ::std::fmt::Display,
        {
            self.email = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for email: {e}"));
            self
        }
        pub fn invited_at<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::option::Option<super::Timestamp>>,
            T::Error: ::std::fmt::Display,
        {
            self.invited_at = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for invited_at: {e}"));
            self
        }
        pub fn joined_at<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::option::Option<super::Timestamp>>,
            T::Error: ::std::fmt::Display,
        {
            self.joined_at = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for joined_at: {e}"));
            self
        }
        pub fn object<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::serde_json::Value>,
            T::Error: ::std::fmt::Display,
        {
            self.object = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for object: {e}"));
            self
        }
        pub fn status<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::WaitlistStatus>,
            T::Error: ::std::fmt::Display,
        {
            self.status = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for status: {e}"));
            self
        }
    }
    impl ::std::convert::TryFrom<WaitlistEntry> for super::WaitlistEntry {
        type Error = super::error::ConversionError;
        fn try_from(
            value: WaitlistEntry,
        ) -> ::std::result::Result<Self, super::error::ConversionError> {
            Ok(Self {
                created_at: value.created_at?,
                email: value.email?,
                invited_at: value.invited_at?,
                joined_at: value.joined_at?,
                object: value.object?,
                status: value.status?,
            })
        }
    }
    impl ::std::convert::From<super::WaitlistEntry> for WaitlistEntry {
        fn from(value: super::WaitlistEntry) -> Self {
            Self {
                created_at: Ok(value.created_at),
                email: Ok(value.email),
                invited_at: Ok(value.invited_at),
                joined_at: Ok(value.joined_at),
                object: Ok(value.object),
                status: Ok(value.status),
            }
        }
    }
    #[derive(Clone, Debug)]
    pub struct WaitlistEntryList {
        data: ::std::result::Result<::std::vec::Vec<super::WaitlistEntry>, ::std::string::String>,
        object: ::std::result::Result<::serde_json::Value, ::std::string::String>,
    }
    impl ::std::default::Default for WaitlistEntryList {
        fn default() -> Self {
            Self {
                data: Err("no value supplied for data".to_string()),
                object: Err("no value supplied for object".to_string()),
            }
        }
    }
    impl WaitlistEntryList {
        pub fn data<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::vec::Vec<super::WaitlistEntry>>,
            T::Error: ::std::fmt::Display,
        {
            self.data = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for data: {e}"));
            self
        }
        pub fn object<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::serde_json::Value>,
            T::Error: ::std::fmt::Display,
        {
            self.object = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for object: {e}"));
            self
        }
    }
    impl ::std::convert::TryFrom<WaitlistEntryList> for super::WaitlistEntryList {
        type Error = super::error::ConversionError;
        fn try_from(
            value: WaitlistEntryList,
        ) -> ::std::result::Result<Self, super::error::ConversionError> {
            Ok(Self {
                data: value.data?,
                object: value.object?,
            })
        }
    }
    impl ::std::convert::From<super::WaitlistEntryList> for WaitlistEntryList {
        fn from(value: super::WaitlistEntryList) -> Self {
            Self {
                data: Ok(value.data),
                object: Ok(value.object),
            }
        }
    }
    #[derive(Clone, Debug)]
    pub struct WaitlistSubmission {
        email: ::std::result::Result<::std::string::String, ::std::string::String>,
        object: ::std::result::Result<::serde_json::Value, ::std::string::String>,
        received_at: ::std::result::Result<super::Timestamp, ::std::string::String>,
        status: ::std::result::Result<::serde_json::Value, ::std::string::String>,
    }
    impl ::std::default::Default for WaitlistSubmission {
        fn default() -> Self {
            Self {
                email: Err("no value supplied for email".to_string()),
                object: Err("no value supplied for object".to_string()),
                received_at: Err("no value supplied for received_at".to_string()),
                status: Err("no value supplied for status".to_string()),
            }
        }
    }
    impl WaitlistSubmission {
        pub fn email<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::string::String>,
            T::Error: ::std::fmt::Display,
        {
            self.email = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for email: {e}"));
            self
        }
        pub fn object<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::serde_json::Value>,
            T::Error: ::std::fmt::Display,
        {
            self.object = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for object: {e}"));
            self
        }
        pub fn received_at<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::Timestamp>,
            T::Error: ::std::fmt::Display,
        {
            self.received_at = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for received_at: {e}"));
            self
        }
        pub fn status<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::serde_json::Value>,
            T::Error: ::std::fmt::Display,
        {
            self.status = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for status: {e}"));
            self
        }
    }
    impl ::std::convert::TryFrom<WaitlistSubmission> for super::WaitlistSubmission {
        type Error = super::error::ConversionError;
        fn try_from(
            value: WaitlistSubmission,
        ) -> ::std::result::Result<Self, super::error::ConversionError> {
            Ok(Self {
                email: value.email?,
                object: value.object?,
                received_at: value.received_at?,
                status: value.status?,
            })
        }
    }
    impl ::std::convert::From<super::WaitlistSubmission> for WaitlistSubmission {
        fn from(value: super::WaitlistSubmission) -> Self {
            Self {
                email: Ok(value.email),
                object: Ok(value.object),
                received_at: Ok(value.received_at),
                status: Ok(value.status),
            }
        }
    }
}
