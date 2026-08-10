//! Running the tool, and the credential that only this process holds.
//!
//! The signature is the design: [`ToolRunner::run`] requires a
//! [`SpendPermit`](crate::spend::SpendPermit) by reference, and that value can
//! only come from [`OrganizationCeiling::admit`](crate::spend::OrganizationCeiling::admit).
//! There is no path to the vendor credential that does not pass the ceiling
//! first, and there is none because the compiler will not build one.

use aex_brain_managed_web::egress::DnsResolver;
use aex_brain_managed_web::search::{
    SearchRejection, WebSearchCredential, WebSearchRequest, search,
};
use aex_internal_contracts::tool_exec::{ArgumentsJcs, ToolResultPart};
use aex_wire::ids::ResourceName;
use async_trait::async_trait;

use crate::spend::SpendPermit;

/// The one tool this executor runs today.
///
/// `web_search` is the only catalogue entry that is both
/// `EgressClass::ManagedInternet` and destined for the platform's own
/// credential. `web_fetch` shares the egress class today and moves to
/// `GuestInternet` when it becomes a sandbox executable, which is a catalogue
/// change rather than a change here.
pub const WEB_SEARCH: &str = "web_search";

/// What running a tool produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolRun {
    /// The result content.
    pub content: Vec<ToolResultPart>,
    /// Whether the tool itself reported failure. A tool that failed is a
    /// *result* the model reads, not a dispatch error.
    pub is_error: bool,
}

/// Why a tool could not be run at all.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RunRefusal {
    /// This executor does not run this tool.
    #[error("`{tool}` is not a tool this executor runs")]
    Unsupported {
        /// Which tool was asked for.
        tool: String,
    },
    /// The argument document did not parse or did not satisfy the tool's shape.
    #[error("the argument document is not valid for this tool")]
    InvalidArguments,
    /// The vendor call failed. Redacted: a provider body never enters an error.
    #[error("the vendor call failed")]
    Vendor,
}

/// Something that runs one already-admitted tool call.
#[async_trait]
pub trait ToolRunner: Send + Sync {
    /// Whether this runner implements `tool`.
    fn supports(&self, tool: &ResourceName) -> bool;

    /// Runs one call.
    ///
    /// The `permit` argument is the point. It is not read for its contents; it
    /// is required so that no caller can reach this function — and therefore the
    /// vendor credential — without having passed the organization ceiling.
    ///
    /// # Errors
    ///
    /// Returns [`RunRefusal`], never a vendor body.
    async fn run(
        &self,
        permit: &SpendPermit,
        tool: &ResourceName,
        arguments: &ArgumentsJcs,
    ) -> Result<ToolRun, RunRefusal>;
}

/// The production runner: platform-paid search over the screened egress policy.
///
/// The credential is unwrapped once at cold start and lives only in this
/// process's memory. It is the reason the deployable exists: while this ran
/// inside `brain-mux`, the platform's vendor key sat in the process that also
/// parses fetched pages, model output and tool results.
pub struct ManagedWebRunner {
    credential: WebSearchCredential,
    resolver: Box<dyn DnsResolver>,
}

impl ManagedWebRunner {
    /// Binds the runner to the platform credential and a resolver.
    #[must_use]
    pub fn new(credential: WebSearchCredential, resolver: Box<dyn DnsResolver>) -> Self {
        Self {
            credential,
            resolver,
        }
    }
}

/// The arguments `web_search` accepts, as the catalogue schema already declares
/// them. Decoded with `deny_unknown_fields` so an argument the Brain validated
/// against a different manifest revision is refused here rather than dropped.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct SearchArguments {
    query: String,
    #[serde(default)]
    count: Option<u8>,
    #[serde(default)]
    country: Option<String>,
}

#[async_trait]
impl ToolRunner for ManagedWebRunner {
    fn supports(&self, tool: &ResourceName) -> bool {
        tool.as_str() == WEB_SEARCH
    }

    async fn run(
        &self,
        _permit: &SpendPermit,
        tool: &ResourceName,
        arguments: &ArgumentsJcs,
    ) -> Result<ToolRun, RunRefusal> {
        if !self.supports(tool) {
            return Err(RunRefusal::Unsupported {
                tool: tool.as_str().to_owned(),
            });
        }
        let parsed: SearchArguments = serde_json::from_slice(arguments.as_bytes())
            .map_err(|_| RunRefusal::InvalidArguments)?;

        let request = WebSearchRequest {
            query: parsed.query.as_str(),
            count: parsed.count.unwrap_or(10),
            country: parsed.country.as_deref(),
            freshness: None,
        };
        match search(request, &self.credential, self.resolver.as_ref()).await {
            Ok(result) => {
                let text = serde_json::to_string(&result).map_err(|_| RunRefusal::Vendor)?;
                Ok(ToolRun {
                    content: vec![ToolResultPart::Text { text }],
                    is_error: false,
                })
            }
            // A refused request is the tool's own verdict and belongs to the
            // model; a transport or vendor fault is ours and belongs to the
            // caller as a dispatch failure. The split is the same one the
            // catalogue already draws between a tool result and a dispatch
            // error.
            Err(
                SearchRejection::Transport
                | SearchRejection::ProviderStatus { .. }
                | SearchRejection::ResponseMalformed
                | SearchRejection::CredentialMalformed,
            ) => Err(RunRefusal::Vendor),
            Err(other) => Ok(ToolRun {
                content: vec![ToolResultPart::Text {
                    text: other.to_string(),
                }],
                is_error: true,
            }),
        }
    }
}
