//! The application context this deployable can honestly supply.
//!
//! `aex_session_app::AppContext` names eleven ports. This deployable owns four
//! of them — the clock, the identifier source, the session authority and the
//! account projection — and the remaining seven belong to streams that have not
//! produced an adapter.
//!
//! [`UnownedPorts`] is **not** a fake adapter. Every method returns
//! [`PortError::Unowned`] naming the seam that owes the implementation. Writing
//! thin local versions instead is the pattern the whole regional stream refused
//! (`references/rewrite/regional-services.md:109-110`): a locally computed
//! `TrueIdle`, for instance, would mean two authorities answering "is this
//! session idle" differently. A refusal is loud, typed and unmountable; an
//! invention is silent and wrong.
//!
//! The three routes this deployable mounts — `session_stop`, `session_trash`
//! and `session_restore` — touch none of the seven, which is what makes
//! mounting them compatible with RS-18.

use aex_content_domain::{
    ContentDigest, ContentOutcome, ContentRoot, PageDigest, TreeNode, TreeView,
};
use aex_secret_domain::{SecretName, SessionCustody, TrueIdle, WorkspaceSecret};
use aex_session_app::ports::{
    ContentReader, ContinuityReader, IdFactory, LimitsReader, LiveWorkspaceReader, PortError,
    RegistryReader, ReservationAuthority, ReservationGrant, ReservationRequest,
    SecretCustodyReader, WorkspaceContinuity,
};
use aex_session_domain::EffectiveLimits;
use aex_wire::ids::{GenerationId, SessionId, UploadId, Uuid7, WorkspaceId};
use aex_wire::limits::LimitId;
use aex_workspace_domain::{RegistryPointer, RegistrySelector, Upload};

/// A time-ordered identifier source.
///
/// Real, not scripted: `uuid` already mints v7 and the workspace's `Uuid7`
/// checks the version and variant nibbles on construction, so an identifier
/// that is not time-ordered cannot escape this function.
#[derive(Debug, Clone, Copy, Default)]
pub struct RequestIds;

impl IdFactory for RequestIds {
    fn next_uuid_v7(&self) -> Uuid7 {
        Uuid7::from_bytes(*uuid::Uuid::now_v7().as_bytes())
            .unwrap_or_else(|_| unreachable!("`Uuid::now_v7` always sets the v7 version nibble"))
    }
}

/// Every port whose adapter another stream owns.
///
/// One type rather than seven so the set is countable, and so a route that
/// starts needing one of them has to delete a refusal rather than quietly find
/// a working implementation.
#[derive(Debug, Clone, Copy, Default)]
pub struct UnownedPorts;

/// The seam each refusal names.
const REGISTRY_SEAM: &str = "the named-registry adapter publishes `RegistryStore`, not \
                             `aex_session_app::ports::RegistryReader`";
const CONTENT_SEAM: &str = "content descriptors and Merkle pages are `aex-content-dynamodb`'s";
const CUSTODY_SEAM: &str = "`aex-secret-custody-dynamodb` exists and is bound here, but no type \
                            implements `SecretCustodyReader` (cluster B2)";
const LIMITS_SEAM: &str = "no authoritative regional capacity default and override producer \
                           exists yet";
const RESERVATION_SEAM: &str = "`ReservationAuthority` is deliberately unowned: the grant and \
                                release protocol belongs to the finance and usage streams";
const CONTINUITY_SEAM: &str = "`aex-runtime-control` owns `TrueIdle` and workspace continuity; \
                               computing them here would give two authorities two answers";
const LIVE_SEAM: &str = "the persist survey manifest and the live workspace scan are Hands' \
                         (cluster B4)";

#[async_trait::async_trait]
impl RegistryReader for UnownedPorts {
    async fn read_many(
        &self,
        _workspace: WorkspaceId,
        _selectors: &[RegistrySelector],
    ) -> Result<Vec<RegistryPointer>, PortError> {
        Err(PortError::Unowned {
            kind: "registry pointers",
            seam: REGISTRY_SEAM,
        })
    }

    async fn read_upload(
        &self,
        _workspace: WorkspaceId,
        _upload: UploadId,
    ) -> Result<Upload, PortError> {
        Err(PortError::Unowned {
            kind: "upload",
            seam: REGISTRY_SEAM,
        })
    }
}

#[async_trait::async_trait]
impl ContentReader for UnownedPorts {
    async fn describe(
        &self,
        _workspace: WorkspaceId,
        _digest: ContentDigest,
    ) -> Result<ContentOutcome, PortError> {
        Err(PortError::Unowned {
            kind: "content descriptor",
            seam: CONTENT_SEAM,
        })
    }

    async fn load_page(
        &self,
        _workspace: WorkspaceId,
        _page: PageDigest,
    ) -> Result<TreeNode, PortError> {
        Err(PortError::Unowned {
            kind: "content page",
            seam: CONTENT_SEAM,
        })
    }
}

#[async_trait::async_trait]
impl SecretCustodyReader for UnownedPorts {
    async fn read_secrets(
        &self,
        _workspace: WorkspaceId,
        _names: &[SecretName],
    ) -> Result<Vec<WorkspaceSecret>, PortError> {
        Err(PortError::Unowned {
            kind: "workspace secrets",
            seam: CUSTODY_SEAM,
        })
    }

    async fn read_custody(&self, _session: SessionId) -> Result<Option<SessionCustody>, PortError> {
        Err(PortError::Unowned {
            kind: "session custody",
            seam: CUSTODY_SEAM,
        })
    }
}

#[async_trait::async_trait]
impl LimitsReader for UnownedPorts {
    async fn effective(
        &self,
        _workspace: WorkspaceId,
        _ids: &[LimitId],
    ) -> Result<EffectiveLimits, PortError> {
        Err(PortError::Unowned {
            kind: "effective limits",
            seam: LIMITS_SEAM,
        })
    }
}

#[async_trait::async_trait]
impl ReservationAuthority for UnownedPorts {
    async fn prepare(&self, _request: ReservationRequest) -> Result<ReservationGrant, PortError> {
        Err(PortError::Unowned {
            kind: "spend reservation",
            seam: RESERVATION_SEAM,
        })
    }
}

#[async_trait::async_trait]
impl ContinuityReader for UnownedPorts {
    async fn continuity(&self, _session: SessionId) -> Result<WorkspaceContinuity, PortError> {
        Err(PortError::Unowned {
            kind: "workspace continuity",
            seam: CONTINUITY_SEAM,
        })
    }

    async fn true_idle(&self, _session: SessionId) -> Result<TrueIdle, PortError> {
        Err(PortError::Unowned {
            kind: "true idle",
            seam: CONTINUITY_SEAM,
        })
    }
}

#[async_trait::async_trait]
impl LiveWorkspaceReader for UnownedPorts {
    async fn scan(
        &self,
        _session: SessionId,
        _generation: GenerationId,
    ) -> Result<TreeView, PortError> {
        Err(PortError::Unowned {
            kind: "live workspace tree",
            seam: LIVE_SEAM,
        })
    }

    async fn root(
        &self,
        _session: SessionId,
        _generation: GenerationId,
    ) -> Result<ContentRoot, PortError> {
        Err(PortError::Unowned {
            kind: "live workspace root",
            seam: LIVE_SEAM,
        })
    }
}
