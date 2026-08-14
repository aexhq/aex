//! Narrow public-head settlement for asynchronous sandbox preparation.
//!
//! Session admission's eager worker and Tool Mux first-call recovery share the
//! private single-materializer lease here. The adapter also owns the authority
//! transition after setup finishes: it strongly reloads the public session,
//! plans the domain checkpoint, and commits one revision-conditional head
//! write. Races reload and re-plan; ambiguous writes are resolved by a strong
//! observation, never a blind retry.

use aex_session_app::ports::{AuthorityCommitter as _, CommitError, PortError};
use aex_session_domain::{LifecycleStatus, SandboxPreparationStatus};
use aex_wire::ids::{GenerationId, OrganizationId, SessionId, WorkspaceId};
use aex_wire::types::Timestamp;
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::types::ReturnValuesOnConditionCheckFailure;

use crate::app_authority::{ApiHintSink, DynamoAuthorityCommitter, SessionCommandReads};
use crate::application_plan::SessionBinding;
use crate::attr::{Item, ItemBuilder, Row, n_i64, s};
use crate::keys;
use crate::plan::RegionalTables;

const MAX_SETTLEMENT_ATTEMPTS: usize = 4;
/// Lease duration shared by eager and first-call materializers.
///
/// The independent heartbeat remains live across content and guest calls, so
/// this is a crash-detection window rather than an I/O timeout. A dead worker
/// can be taken over within ninety seconds.
pub const SANDBOX_PREPARATION_LEASE_MILLIS: i64 = 90 * 1_000;
/// Maximum interval between lease renewals while content or guest I/O is live.
pub const SANDBOX_PREPARATION_HEARTBEAT_MILLIS: u64 = 15 * 1_000;
const PREPARATION_LEASE_ITEM: &str = "sandbox_preparation_lease";

const WORKSPACE: &str = "workspaceId";
const ORGANIZATION: &str = "organizationId";
const SESSION: &str = "sessionId";
const GENERATION: &str = "generationId";
const OWNER: &str = "owner";
const LEASE_UNTIL: &str = "leaseUntilMillis";

/// One fenced lease held by the sole startup-file materializer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxPreparationLease {
    /// Workspace owning the preparation.
    pub workspace: WorkspaceId,
    /// Organization owning the preparation.
    pub organization: OrganizationId,
    /// Session owning the preparation.
    pub session: SessionId,
    /// Exact generation this claim may prepare.
    pub generation: GenerationId,
    /// Unique invocation token. It is never shared by concurrent tasks.
    pub owner: String,
    /// Explicit expiry used for crash recovery; DynamoDB TTL is not a fence.
    pub lease_until: Timestamp,
}

/// Result of trying to become the sole startup-file materializer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxPreparationClaim {
    /// This invocation owns the exact lease.
    Acquired(SandboxPreparationLease),
    /// Another live invocation owns it until the returned instant.
    Contended {
        /// Current owner's explicit crash-recovery deadline.
        lease_until: Timestamp,
    },
}

/// Observed result of publishing the preparation checkpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SandboxPreparationSettlement {
    /// Durable checkpoint observed after the attempt.
    pub checkpoint: SandboxPreparationStatus,
    /// Whether this caller committed the checkpoint.
    pub advanced: bool,
}

/// Why completed sandbox preparation could not update the public head.
#[derive(Debug, thiserror::Error)]
pub enum SandboxPreparationAuthorityError {
    /// The session head could not be read faithfully.
    #[error(transparent)]
    Read(#[from] PortError),
    /// The head does not belong to the elected preparation.
    #[error("the session head does not match the elected sandbox preparation")]
    Binding,
    /// Session termination won the race with setup/tool readiness.
    #[error("the session no longer admits sandbox preparation")]
    SessionClosed,
    /// The application state machine refused the checkpoint.
    #[error(transparent)]
    Plan(#[from] aex_session_app::AppError),
    /// The provider could not establish the commit result.
    #[error(transparent)]
    Commit(CommitError),
    /// Concurrent session activity kept moving the head revision.
    #[error("sandbox preparation settlement remained contended")]
    Contended,
    /// The private materialization lease could not be read or mutated.
    #[error("sandbox preparation lease authority is unavailable")]
    LeaseTransport,
    /// The private materialization lease row is malformed or crosses identity.
    #[error("sandbox preparation lease authority is corrupt")]
    LeaseCorrupt,
    /// This invocation no longer owns the materialization lease.
    #[error("sandbox preparation lease was lost")]
    LeaseLost,
}

/// Strong, conditionally committed session checkpoint used by Tool Mux.
#[derive(Debug, Clone)]
pub struct SandboxPreparationAuthority {
    client: Client,
    tables: RegionalTables,
    reads: SessionCommandReads,
}

impl SandboxPreparationAuthority {
    /// Binds the authority only to the physical `session-authority` table.
    ///
    /// The internal application compiler accepts the regional table catalog,
    /// but this adapter's closed plan membership contains one session-head
    /// write. Unowned table names therefore remain unconfigured rather than
    /// widening Tool Mux's composition surface.
    #[must_use]
    pub fn new(client: Client, session_table: String) -> Self {
        let reads = SessionCommandReads::new(client.clone(), session_table.clone());
        let tables = RegionalTables {
            session_authority: session_table,
            regional_work: String::new(),
            regional_content: String::new(),
            regional_registry: String::new(),
            regional_secret_custody: String::new(),
            regional_secret_keystore: String::new(),
            runtime_activity: String::new(),
            regional_authz_projection: String::new(),
        };
        Self {
            client,
            tables,
            reads,
        }
    }

    /// Strongly observes the exact elected session's preparation checkpoint.
    ///
    /// Eager setup and first-call recovery must call this before startup-file
    /// upload. `Ready` or `Suspended` means user workspace state may already
    /// have diverged from the registered initial files and must never be
    /// rematerialized.
    ///
    /// # Errors
    ///
    /// Returns [`SandboxPreparationAuthorityError`] when the head is absent,
    /// closed, corrupt, or crosses the elected tenant/generation.
    pub async fn observe(
        &self,
        workspace: WorkspaceId,
        organization: OrganizationId,
        session: SessionId,
        generation: GenerationId,
    ) -> Result<SandboxPreparationStatus, SandboxPreparationAuthorityError> {
        Ok(self
            .read_bound(workspace, organization, session, generation)
            .await?
            .lifecycle
            .sandbox)
    }

    /// Conditionally acquires the private single-materializer lease.
    ///
    /// A unique `owner` is required for every invocation. An expired lease can
    /// be taken over for crash recovery; a current-generation contender only
    /// observes the existing deadline and performs no guest I/O.
    pub async fn claim(
        &self,
        workspace: WorkspaceId,
        organization: OrganizationId,
        session: SessionId,
        generation: GenerationId,
        owner: &str,
        now: Timestamp,
    ) -> Result<SandboxPreparationClaim, SandboxPreparationAuthorityError> {
        let lease_until = lease_deadline(now);
        let head = self
            .read_bound(workspace, organization, session, generation)
            .await?;
        if head.lifecycle.sandbox != SandboxPreparationStatus::Requested
            || owner.is_empty()
            || lease_until <= now
        {
            return Err(SandboxPreparationAuthorityError::LeaseLost);
        }
        let key = keys::sandbox_preparation_lease(session);
        let item = lease_item(
            &key,
            workspace,
            organization,
            session,
            generation,
            owner,
            lease_until,
        );
        let request = self
            .client
            .put_item()
            .table_name(&self.tables.session_authority)
            .set_item(Some(item))
            .condition_expression(
                "attribute_not_exists(#pk) OR #generation <> :generation OR #leaseUntil < :now",
            )
            .expression_attribute_names("#pk", "pk")
            .expression_attribute_names("#generation", GENERATION)
            .expression_attribute_names("#leaseUntil", LEASE_UNTIL)
            .expression_attribute_values(":generation", s(generation.to_string()))
            .expression_attribute_values(":now", n_i64(now.unix_millis()))
            .return_values_on_condition_check_failure(ReturnValuesOnConditionCheckFailure::AllOld)
            .send()
            .await;
        match request {
            Ok(_) => Ok(SandboxPreparationClaim::Acquired(SandboxPreparationLease {
                workspace,
                organization,
                session,
                generation,
                owner: owner.to_owned(),
                lease_until,
            })),
            Err(error) if conditional_put(&error) => {
                let observed = self
                    .read_lease(workspace, organization, session)
                    .await?
                    .ok_or(SandboxPreparationAuthorityError::LeaseTransport)?;
                if observed.generation != generation {
                    return Err(SandboxPreparationAuthorityError::LeaseCorrupt);
                }
                Ok(SandboxPreparationClaim::Contended {
                    lease_until: observed.lease_until,
                })
            }
            Err(_) => {
                // A timed-out put can still have committed. Resolve it with a
                // strong read before reporting an unknown outcome.
                match self.read_lease(workspace, organization, session).await? {
                    Some(observed)
                        if observed.generation == generation && observed.owner == owner =>
                    {
                        Ok(SandboxPreparationClaim::Acquired(observed))
                    }
                    Some(observed) if observed.generation == generation => {
                        Ok(SandboxPreparationClaim::Contended {
                            lease_until: observed.lease_until,
                        })
                    }
                    _ => Err(SandboxPreparationAuthorityError::LeaseTransport),
                }
            }
        }
    }

    /// Renews an owned lease before the next bounded guest transfer.
    pub async fn renew(
        &self,
        session: SessionId,
        lease: &mut SandboxPreparationLease,
        now: Timestamp,
    ) -> Result<(), SandboxPreparationAuthorityError> {
        if lease.session != session {
            return Err(SandboxPreparationAuthorityError::LeaseCorrupt);
        }
        let lease_until = lease_deadline(now);
        if lease_until == lease.lease_until {
            return Ok(());
        }
        if lease_until < lease.lease_until {
            return Err(SandboxPreparationAuthorityError::LeaseLost);
        }
        let key = keys::sandbox_preparation_lease(session);
        let result = self
            .client
            .update_item()
            .table_name(&self.tables.session_authority)
            .key("pk", s(key.pk))
            .key("sk", s(key.sk))
            .update_expression("SET #leaseUntil = :leaseUntil")
            .condition_expression("#generation = :generation AND #owner = :owner")
            .expression_attribute_names("#leaseUntil", LEASE_UNTIL)
            .expression_attribute_names("#generation", GENERATION)
            .expression_attribute_names("#owner", OWNER)
            .expression_attribute_values(":leaseUntil", n_i64(lease_until.unix_millis()))
            .expression_attribute_values(":generation", s(lease.generation.to_string()))
            .expression_attribute_values(":owner", s(lease.owner.clone()))
            .send()
            .await;
        match result {
            Ok(_) => {
                lease.lease_until = lease_until;
                Ok(())
            }
            Err(error) if conditional_update(&error) => {
                Err(SandboxPreparationAuthorityError::LeaseLost)
            }
            Err(_) => {
                let observed = self
                    .read_lease(lease.workspace, lease.organization, session)
                    .await?;
                if observed.as_ref().is_some_and(|observed| {
                    observed.generation == lease.generation
                        && observed.owner == lease.owner
                        && observed.lease_until >= lease_until
                }) {
                    lease.lease_until = lease_until;
                    Ok(())
                } else {
                    Err(SandboxPreparationAuthorityError::LeaseTransport)
                }
            }
        }
    }

    /// Releases an owned lease after the public checkpoint is durable.
    pub async fn release(
        &self,
        session: SessionId,
        lease: &SandboxPreparationLease,
    ) -> Result<(), SandboxPreparationAuthorityError> {
        if lease.session != session {
            return Err(SandboxPreparationAuthorityError::LeaseCorrupt);
        }
        let key = keys::sandbox_preparation_lease(session);
        let result = self
            .client
            .delete_item()
            .table_name(&self.tables.session_authority)
            .key("pk", s(key.pk))
            .key("sk", s(key.sk))
            .condition_expression("#generation = :generation AND #owner = :owner")
            .expression_attribute_names("#generation", GENERATION)
            .expression_attribute_names("#owner", OWNER)
            .expression_attribute_values(":generation", s(lease.generation.to_string()))
            .expression_attribute_values(":owner", s(lease.owner.clone()))
            .send()
            .await;
        match result {
            Ok(_) => Ok(()),
            Err(error) if conditional_delete(&error) => {
                Err(SandboxPreparationAuthorityError::LeaseLost)
            }
            Err(_) => match self
                .read_lease(lease.workspace, lease.organization, session)
                .await?
            {
                None => Ok(()),
                Some(observed)
                    if observed.generation != lease.generation || observed.owner != lease.owner =>
                {
                    Err(SandboxPreparationAuthorityError::LeaseLost)
                }
                Some(_) => Err(SandboxPreparationAuthorityError::LeaseTransport),
            },
        }
    }

    /// Publishes successful materialization as `Ready` or `Suspended`.
    ///
    /// `suspended` is true only when Runtime Control proved the exact generation
    /// retained and stopped after setup. A concurrent Brain/session update is
    /// reloaded and the checkpoint is re-planned against its new revision.
    ///
    /// # Errors
    ///
    /// Returns [`SandboxPreparationAuthorityError`] when the head crosses the
    /// elected tenant/generation, cannot be decoded, remains contended, or the
    /// provider cannot establish a commit result.
    pub async fn settle(
        &self,
        workspace: WorkspaceId,
        organization: OrganizationId,
        session: SessionId,
        generation: GenerationId,
        suspended: bool,
        now: Timestamp,
    ) -> Result<SandboxPreparationSettlement, SandboxPreparationAuthorityError> {
        for attempt in 0..MAX_SETTLEMENT_ATTEMPTS {
            let head = self
                .read_bound(workspace, organization, session, generation)
                .await?;
            if head.lifecycle.sandbox != SandboxPreparationStatus::Requested {
                return Ok(SandboxPreparationSettlement {
                    checkpoint: head.lifecycle.sandbox,
                    advanced: false,
                });
            }
            let planned = aex_session_app::settle_sandbox_preparation(&head, suspended, now)?;
            if planned.plan.conditions.is_empty() && planned.plan.writes.is_empty() {
                return Ok(SandboxPreparationSettlement {
                    checkpoint: planned.projected.lifecycle.sandbox,
                    advanced: false,
                });
            }
            let committer = DynamoAuthorityCommitter::new(
                self.client.clone(),
                self.tables.clone(),
                SessionBinding {
                    workspace,
                    organization,
                    session,
                },
                now,
                ApiHintSink,
            );
            match committer.commit(&planned.plan).await {
                Ok(_) => {
                    return Ok(SandboxPreparationSettlement {
                        checkpoint: planned.projected.lifecycle.sandbox,
                        advanced: true,
                    });
                }
                Err(CommitError::ConditionFailed { .. })
                    if attempt + 1 < MAX_SETTLEMENT_ATTEMPTS =>
                {
                    continue;
                }
                Err(CommitError::ConditionFailed { .. }) => {
                    return Err(SandboxPreparationAuthorityError::Contended);
                }
                Err(error @ CommitError::Ambiguous { .. }) => {
                    let observed = self
                        .read_bound(workspace, organization, session, generation)
                        .await?;
                    if observed.lifecycle.sandbox != SandboxPreparationStatus::Requested {
                        return Ok(SandboxPreparationSettlement {
                            checkpoint: observed.lifecycle.sandbox,
                            advanced: false,
                        });
                    }
                    return Err(SandboxPreparationAuthorityError::Commit(error));
                }
                Err(error) => return Err(SandboxPreparationAuthorityError::Commit(error)),
            }
        }
        Err(SandboxPreparationAuthorityError::Contended)
    }

    async fn read_bound(
        &self,
        workspace: WorkspaceId,
        organization: OrganizationId,
        session: SessionId,
        generation: GenerationId,
    ) -> Result<aex_session_domain::Session, SandboxPreparationAuthorityError> {
        let head = self.reads.load_session_strong(workspace, session).await?;
        if head.organization != organization
            || head.id != session
            || head.generation != Some(generation)
            || head.lifecycle.generation != Some(generation)
        {
            return Err(SandboxPreparationAuthorityError::Binding);
        }
        if matches!(
            head.lifecycle.status,
            LifecycleStatus::Terminating | LifecycleStatus::Terminated | LifecycleStatus::Deleting
        ) || head.lifecycle.sandbox == SandboxPreparationStatus::Lost
        {
            return Err(SandboxPreparationAuthorityError::SessionClosed);
        }
        Ok(head)
    }

    async fn read_lease(
        &self,
        workspace: WorkspaceId,
        organization: OrganizationId,
        session: SessionId,
    ) -> Result<Option<SandboxPreparationLease>, SandboxPreparationAuthorityError> {
        let key = keys::sandbox_preparation_lease(session);
        let output = self
            .client
            .get_item()
            .table_name(&self.tables.session_authority)
            .key("pk", s(key.pk))
            .key("sk", s(key.sk))
            .consistent_read(true)
            .send()
            .await
            .map_err(|_| SandboxPreparationAuthorityError::LeaseTransport)?;
        output
            .item
            .as_ref()
            .map(|item| decode_lease(item, workspace, organization, session))
            .transpose()
    }
}

fn lease_item(
    key: &keys::Key,
    workspace: WorkspaceId,
    organization: OrganizationId,
    session: SessionId,
    generation: GenerationId,
    owner: &str,
    lease_until: Timestamp,
) -> Item {
    ItemBuilder::new(PREPARATION_LEASE_ITEM)
        .set("pk", s(key.pk.clone()))
        .set("sk", s(key.sk.clone()))
        .set(WORKSPACE, s(workspace.to_string()))
        .set(ORGANIZATION, s(organization.to_string()))
        .set(SESSION, s(session.to_string()))
        .set(GENERATION, s(generation.to_string()))
        .set(OWNER, s(owner.to_owned()))
        .set(LEASE_UNTIL, n_i64(lease_until.unix_millis()))
        .build()
}

fn decode_lease(
    item: &Item,
    expected_workspace: WorkspaceId,
    expected_organization: OrganizationId,
    expected_session: SessionId,
) -> Result<SandboxPreparationLease, SandboxPreparationAuthorityError> {
    let row = Row::bind(item, PREPARATION_LEASE_ITEM)
        .map_err(|_| SandboxPreparationAuthorityError::LeaseCorrupt)?;
    let generation = row
        .string(GENERATION)
        .map_err(|_| SandboxPreparationAuthorityError::LeaseCorrupt)?
        .parse::<GenerationId>()
        .map_err(|_| SandboxPreparationAuthorityError::LeaseCorrupt)?;
    let workspace = row
        .string(WORKSPACE)
        .map_err(|_| SandboxPreparationAuthorityError::LeaseCorrupt)?
        .parse::<WorkspaceId>()
        .map_err(|_| SandboxPreparationAuthorityError::LeaseCorrupt)?;
    let organization = row
        .string(ORGANIZATION)
        .map_err(|_| SandboxPreparationAuthorityError::LeaseCorrupt)?
        .parse::<OrganizationId>()
        .map_err(|_| SandboxPreparationAuthorityError::LeaseCorrupt)?;
    let session = row
        .string(SESSION)
        .map_err(|_| SandboxPreparationAuthorityError::LeaseCorrupt)?
        .parse::<SessionId>()
        .map_err(|_| SandboxPreparationAuthorityError::LeaseCorrupt)?;
    if workspace != expected_workspace
        || organization != expected_organization
        || session != expected_session
    {
        return Err(SandboxPreparationAuthorityError::LeaseCorrupt);
    }
    let owner = row
        .string(OWNER)
        .map_err(|_| SandboxPreparationAuthorityError::LeaseCorrupt)?
        .to_owned();
    let millis = row
        .item()
        .get(LEASE_UNTIL)
        .and_then(|value| value.as_n().ok())
        .and_then(|value| value.parse::<i64>().ok())
        .ok_or(SandboxPreparationAuthorityError::LeaseCorrupt)?;
    let lease_until = Timestamp::from_unix_millis(millis)
        .map_err(|_| SandboxPreparationAuthorityError::LeaseCorrupt)?;
    Ok(SandboxPreparationLease {
        workspace,
        organization,
        session,
        generation,
        owner,
        lease_until,
    })
}

fn conditional_put<R>(
    error: &aws_sdk_dynamodb::error::SdkError<
        aws_sdk_dynamodb::operation::put_item::PutItemError,
        R,
    >,
) -> bool {
    error.as_service_error().is_some_and(|service| {
        matches!(
            service,
            aws_sdk_dynamodb::operation::put_item::PutItemError::ConditionalCheckFailedException(_)
        )
    })
}

fn conditional_update<R>(
    error: &aws_sdk_dynamodb::error::SdkError<
        aws_sdk_dynamodb::operation::update_item::UpdateItemError,
        R,
    >,
) -> bool {
    error.as_service_error().is_some_and(|service| {
        matches!(
            service,
            aws_sdk_dynamodb::operation::update_item::UpdateItemError::ConditionalCheckFailedException(_)
        )
    })
}

fn conditional_delete<R>(
    error: &aws_sdk_dynamodb::error::SdkError<
        aws_sdk_dynamodb::operation::delete_item::DeleteItemError,
        R,
    >,
) -> bool {
    error.as_service_error().is_some_and(|service| {
        matches!(
            service,
            aws_sdk_dynamodb::operation::delete_item::DeleteItemError::ConditionalCheckFailedException(_)
        )
    })
}

fn lease_deadline(now: Timestamp) -> Timestamp {
    Timestamp::from_unix_millis(
        now.unix_millis()
            .saturating_add(SANDBOX_PREPARATION_LEASE_MILLIS),
    )
    .unwrap_or(now)
}

#[cfg(test)]
mod tests {
    use super::{
        SANDBOX_PREPARATION_HEARTBEAT_MILLIS, SANDBOX_PREPARATION_LEASE_MILLIS, lease_deadline,
    };
    use aex_wire::types::Timestamp;

    #[test]
    fn a_dead_materializer_becomes_takeover_eligible_within_ninety_seconds() {
        let now = Timestamp::from_unix_millis(1_000).expect("time");
        assert_eq!(
            lease_deadline(now).unix_millis() - now.unix_millis(),
            90_000
        );
        assert_eq!(SANDBOX_PREPARATION_LEASE_MILLIS, 90_000);
    }

    #[test]
    fn several_heartbeats_fit_inside_one_crash_detection_window() {
        assert!(
            i64::try_from(SANDBOX_PREPARATION_HEARTBEAT_MILLIS).expect("heartbeat fits") * 4
                < SANDBOX_PREPARATION_LEASE_MILLIS
        );
    }
}
