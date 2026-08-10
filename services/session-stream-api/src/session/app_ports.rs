//! The application context this deployable can honestly supply.
//!
//! `aex_session_app::AppContext` names eleven ports. This deployable owns six
//! of them — the clock, the identifier source, the session authority, the
//! account projection, runtime continuity and secret custody — and the
//! remaining five belong to streams that have not produced an adapter.
//!
//! Two ports left this file rather than gaining a local implementation, and
//! both for the same reason: the authority already existed elsewhere.
//! `aex_runtime_activity_dynamodb::RuntimeContinuity` reads the rows
//! `aex-runtime-control` owns the idle predicate over, so there is no second
//! reading of "is this session idle";
//! `aex_secret_custody_dynamodb::SessionCustodyReads` holds both rows a custody
//! read has to join, so the sealed row keeps exactly one reader.
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
//! and `session_restore` — touch none of the five, which is what makes mounting
//! them compatible with RS-18.
//!
//! `session_credential_rebind` can now **read** everything it needs and still
//! cannot be mounted, for a reason on the write side: its plan's
//! `Write::PutCustody` names one item, while the stored shape is a head row
//! plus one immutable row per bound name. The binding rows are not expressible
//! in the closed write vocabulary at all, and the compiler's "one logical item,
//! one provider action" rule means they cannot be smuggled into the head write.
//! That is a custody-write question and it belongs to whoever owns the custody
//! transaction, not to the session command path.

use aex_content_domain::{
    ContentDigest, ContentOutcome, ContentRoot, PageDigest, TreeNode, TreeView,
};
use aex_session_app::ports::{
    ContentReader, IdFactory, LimitsReader, LiveEntry, LiveListQuery, LiveListing,
    LiveWorkspaceReader, PortError, RegistryReader, ReservationAuthority, ReservationGrant,
    ReservationRequest,
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
const LIMITS_SEAM: &str = "no authoritative regional capacity default and override producer \
                           exists yet";
const RESERVATION_SEAM: &str = "`ReservationAuthority` is deliberately unowned: the grant and \
                                release protocol belongs to the finance and usage streams";
const LIVE_SEAM: &str = "the persist survey manifest and the live workspace scan are Hands': \
                         nothing in the tree produces a live `TreeView`, and the guest refuses \
                         `PersistPhase::Survey` for want of a TLS client";

/// Why the two **observation** methods are refused, which is a different reason
/// from [`LIVE_SEAM`].
///
/// Not hashing, and not the guest. The guest already answers a listing and a
/// stat from `lstat` alone — `aex_hands_tools::filesystem::list_dir` and
/// `stat_path`, dispatched by `runtimes/hands-agent/src/execute.rs` — with no
/// digest, no Merkle build and no TLS. What is missing is entirely on this
/// side: reaching a running generation needs the authenticated guest transport
/// (`aex_brain_hands::HttpGuestTransport`) over a provider endpoint and token
/// minted by `MicrovmControlApi::auth_token`, and this deployable composes
/// neither. Composing them here would also hand a `public_edge` service the
/// same trait that carries `run`, `resume` and `terminate`, which is a
/// lifecycle-authority decision and not a file-read one.
const LIVE_OBSERVATION_SEAM: &str = "the guest answers a listing from `lstat` with no digest; what is absent is the authenticated \
     guest transport and the provider endpoint token, which this deployable does not compose";

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

    async fn list(
        &self,
        _session: SessionId,
        _generation: GenerationId,
        _query: &LiveListQuery,
    ) -> Result<LiveListing, PortError> {
        Err(PortError::Unowned {
            kind: "live workspace listing",
            seam: LIVE_OBSERVATION_SEAM,
        })
    }

    async fn stat(
        &self,
        _session: SessionId,
        _generation: GenerationId,
        _path: &str,
    ) -> Result<LiveEntry, PortError> {
        Err(PortError::Unowned {
            kind: "live workspace entry",
            seam: LIVE_OBSERVATION_SEAM,
        })
    }
}
