//! The verbatim `regional-secret-custody` condition and update expressions.
//!
//! Revocation is deliberately **one conditional update on one item**. It blocks
//! every prior source generation for a name — including generations already
//! copied into sessions and clones — because every use path conditions on
//! `revokedThroughRevision < :boundRevision`. Marking individual generation rows
//! is a lazy audit sweep and is never the fence. That keeps an emergency revoke
//! O(1) whatever the generation count is, which matters because a fan-out over
//! generations would exceed the 100-action transaction ceiling exactly when it
//! was needed most.

use aex_secret_domain::custody::CustodyRevision;
use aex_secret_domain::revocation::RevocationEpoch;
use aex_secret_domain::secret::{SecretRevision, SecretState, SourceGeneration};
use aex_session_dynamodb::attr::{Item, n, s, stamp};
use aex_session_dynamodb::error::StoreError;
use aex_session_dynamodb::plan::{IMMUTABLE, Participant, TransactionPlan, key};
use aex_session_dynamodb::replay::{Receipt, encode_receipt_row};
use aex_wire::ids::{SessionId, WorkspaceId};
use aex_wire::types::Timestamp;
use aws_sdk_dynamodb::types::builders::UpdateBuilder;
use aws_sdk_dynamodb::types::{ConditionCheck, Put, Update};

use crate::codec::{
    self, CallAuthorization, CredentialState, ProviderCredential, SecretMetadata, StoredGeneration,
    secret_state_str,
};
use crate::keys;

/// A transport deduplication identity inside the provider's 36-character
/// ceiling.
#[must_use]
pub fn token(tag: &str, parts: &[&str]) -> String {
    use sha2::Digest as _;
    let mut hasher = sha2::Sha256::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update([0x1f]);
    }
    let digest = hex::encode(hasher.finalize());
    let tag: String = tag.chars().take(3).collect();
    format!("{tag}-{}", &digest[..32])
}

/// The participants a secret `set` names, in plan order.
///
/// This is the **minimum** set: the two optional participants a first-seal write
/// adds — the durable receipt (D-6) and the quota guard (D-13) — append in the
/// order [`set`] compiles them, so a plan's participant list is exactly what the
/// caller asked for and never a superset.
pub const SET_ORDER: [Participant; 3] = [
    Participant::SECRET_GENERATION,
    Participant::SECRET_METADATA,
    Participant::SECRET_LINEAGE,
];

/// A per-workspace collection bound, enforced by a counter row (D-13).
///
/// The guard is a participant of the same transaction as the write it bounds,
/// so a create that would exceed the maximum commits **nothing** — no
/// generation, no metadata, no lineage and no receipt. Counting by listing was
/// rejected: it is `O(n)` on a write path, and `list_secrets` typed-refuses a
/// collection past its page budget, so it would start failing before the limit
/// did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quota {
    /// Which counter is being consumed.
    pub participant: Participant,
    /// The largest count the workspace may reach.
    pub max: u64,
}

impl Quota {
    /// The secret-count bound of one workspace.
    #[must_use]
    pub const fn secrets(max: u64) -> Self {
        Self {
            participant: Participant::SECRET_COUNT,
            max,
        }
    }

    /// The provider-credential-count bound of one workspace.
    #[must_use]
    pub const fn provider_credentials(max: u64) -> Self {
        Self {
            participant: Participant::CUSTODY_CREDENTIAL_COUNT,
            max,
        }
    }
}

/// Compiles the secret `set` transaction.
///
/// The generation row is written first and the metadata second, so a cancelled
/// transaction can never leave metadata pointing at a generation that does not
/// exist.
///
/// `receipt` is `Some` for every route that declares an `Idempotency-Key`, and
/// the receipt `Put` is a participant of **this** transaction rather than a
/// second one: a receipt written separately reintroduces exactly the ambiguous
/// window it exists to close.
///
/// `quota` is `Some` only when the handler observed no existing record — a
/// replace does not consume quota, because the collection does not grow.
///
/// # Errors
///
/// [`StoreError`] when a row could not be encoded or an action could not be
/// built, or when `quota` is supplied for a replace.
pub fn set(
    table: &str,
    generation: &StoredGeneration,
    metadata: &SecretMetadata,
    expected_revision: Option<SecretRevision>,
    receipt: Option<&Receipt>,
    quota: Option<Quota>,
) -> Result<TransactionPlan, StoreError> {
    validate_set(generation, metadata, expected_revision)?;
    if quota.is_some() && expected_revision.is_some() {
        return Err(StoreError::Invalid {
            detail: "a replace does not consume quota; the collection does not grow".to_owned(),
        });
    }
    let mut plan = TransactionPlan::new(token(
        "sec",
        &[
            &metadata.workspace.to_string(),
            metadata.name.as_str(),
            &metadata.revision.0.to_string(),
        ],
    ));
    plan.put(
        Participant::SECRET_GENERATION,
        Put::builder()
            .table_name(table)
            .set_item(Some(codec::encode_generation(generation).map_err(
                |error| StoreError::Invalid {
                    detail: error.to_string(),
                },
            )?))
            .condition_expression(IMMUTABLE),
    )?;

    let metadata_key = keys::secret(metadata.workspace, metadata.name.as_str())?;
    let condition = if expected_revision.is_some() {
        "#state = :ready AND revision = :expectedRevision"
    } else {
        "attribute_not_exists(pk)"
    };
    let mut update = Update::builder()
        .table_name(table)
        .set_key(Some(key(&metadata_key.pk, &metadata_key.sk)))
        .condition_expression(condition)
        // The metadata row is written by an `Update` rather than a `Put` because
        // it carries a monotone revision the caller conditions on. An update
        // that *creates* a row writes exactly the attributes it names, so the
        // discriminator has to be one of them: without it the row decodes as
        // `Missing { attribute: "itemType" }` the first time anyone lists, which
        // is what the engine-backed target caught.
        .update_expression(
            "SET #itemType = :itemType, \
             revision = if_not_exists(revision, :zero) + :one, #state = :ready, \
             activeSourceGeneration = :generation, \
             revocationEpoch = if_not_exists(revocationEpoch, :zero), \
             revokedThroughRevision = if_not_exists(revokedThroughRevision, :zero), \
             #name = :name, workspaceId = :workspace, \
             createdAt = if_not_exists(createdAt, :now), updatedAt = :now \
             REMOVE revokedAt",
        )
        .expression_attribute_names("#state", "state")
        .expression_attribute_names("#name", "name")
        .expression_attribute_names("#itemType", "itemType")
        .expression_attribute_values(":itemType", s(codec::WORKSPACE_SECRET))
        .expression_attribute_values(":ready", s(secret_state_str(metadata.state)))
        .expression_attribute_values(":generation", n(metadata.generation.0))
        .expression_attribute_values(":name", s(metadata.name.as_str().to_owned()))
        .expression_attribute_values(":workspace", s(metadata.workspace.to_string()))
        .expression_attribute_values(":zero", n(0))
        .expression_attribute_values(":one", n(1))
        .expression_attribute_values(":now", stamp(metadata.updated_at));
    if let Some(revision) = expected_revision {
        update = update.expression_attribute_values(":expectedRevision", n(revision.0));
    }
    plan.update(Participant::SECRET_METADATA, update)?;

    plan.put(
        Participant::SECRET_LINEAGE,
        Put::builder()
            .table_name(table)
            .set_item(Some(
                codec::encode_lineage(
                    metadata.workspace,
                    &metadata.name,
                    metadata.generation,
                    metadata.created_at,
                )
                .map_err(|error| StoreError::Invalid {
                    detail: error.to_string(),
                })?,
            ))
            .condition_expression(IMMUTABLE),
    )?;

    if let Some(receipt) = receipt {
        push_receipt(&mut plan, table, metadata.workspace, receipt)?;
    }
    if let Some(quota) = quota {
        push_quota(
            &mut plan,
            table,
            &keys::secret_counter(metadata.workspace),
            metadata.workspace,
            quota,
            metadata.updated_at,
        )?;
    }
    Ok(plan)
}

/// Appends the durable receipt as a participant of the caller's transaction.
///
/// `attribute_not_exists(pk)` **is** the replay election: the loser of a race
/// writes nothing at all, because `TransactWriteItems` is all-or-nothing, and
/// `commit_or_replay` resolves the loss by exactly one receipt re-read.
fn push_receipt(
    plan: &mut TransactionPlan,
    table: &str,
    workspace: WorkspaceId,
    receipt: &Receipt,
) -> Result<(), StoreError> {
    plan.put(
        Participant::SECRET_IDEMPOTENCY,
        Put::builder()
            .table_name(table)
            .set_item(Some(encode_receipt_row(workspace, receipt).map_err(
                |error| StoreError::Invalid {
                    detail: error.to_string(),
                },
            )?))
            .condition_expression(IMMUTABLE),
    )?;
    Ok(())
}

/// Appends the per-workspace counter guard (D-13).
///
/// `ADD count :one` on a row that does not exist yet starts it at one, so the
/// first create needs no separate initialisation. The guard is
/// `attribute_not_exists(#count) OR #count < :max`, which is what makes the
/// bound refuse the `(max + 1)`-th create rather than the `max`-th.
fn push_quota(
    plan: &mut TransactionPlan,
    table: &str,
    counter: &keys::Key,
    workspace: WorkspaceId,
    quota: Quota,
    now: Timestamp,
) -> Result<(), StoreError> {
    plan.update(
        quota.participant,
        Update::builder()
            .table_name(table)
            .set_key(Some(key(&counter.pk, &counter.sk)))
            .condition_expression("attribute_not_exists(#count) OR #count < :max")
            // An `Update` that creates a row writes exactly the attributes it
            // names, so the discriminator has to be one of them or the row
            // decodes as `Missing { attribute: "itemType" }` the first time
            // anything reads the partition.
            .update_expression(
                "SET #itemType = :itemType, workspaceId = :workspace, updatedAt = :now \
                 ADD #count :one",
            )
            .expression_attribute_names("#count", "count")
            .expression_attribute_names("#itemType", "itemType")
            .expression_attribute_values(":itemType", s(keys::CUSTODY_COUNTER))
            .expression_attribute_values(":workspace", s(workspace.to_string()))
            .expression_attribute_values(":max", n(quota.max))
            .expression_attribute_values(":one", n(1))
            .expression_attribute_values(":now", stamp(now)),
    )?;
    Ok(())
}

fn validate_set(
    generation: &StoredGeneration,
    metadata: &SecretMetadata,
    expected_revision: Option<SecretRevision>,
) -> Result<(), StoreError> {
    let target_revision = match expected_revision {
        None => SecretRevision::FIRST,
        Some(revision) => {
            SecretRevision(
                revision
                    .0
                    .checked_add(1)
                    .ok_or_else(|| StoreError::Invalid {
                        detail: "the observed secret revision cannot be advanced".to_owned(),
                    })?,
            )
        }
    };
    if metadata.revision != target_revision {
        return Err(StoreError::Invalid {
            detail: format!(
                "the set plan targets revision {} after observing {}; expected {}",
                metadata.revision.0,
                expected_revision.map_or(0, |revision| revision.0),
                target_revision.0
            ),
        });
    }
    if metadata.state != SecretState::Ready {
        return Err(StoreError::Invalid {
            detail: "a secret set must produce ready metadata".to_owned(),
        });
    }
    if generation.workspace != metadata.workspace
        || generation.name != metadata.name
        || generation.generation != metadata.generation
    {
        return Err(StoreError::Invalid {
            detail: "the sealed generation identity does not match the metadata pointer".to_owned(),
        });
    }
    Ok(())
}

/// Builds the emergency revoke: one atomic fence, O(1) in the generation count.
///
/// # Errors
///
/// [`StoreError`] when the name could not enter a key.
pub fn revoke(
    table: &str,
    workspace: WorkspaceId,
    name: &str,
    expected_epoch: RevocationEpoch,
    now: Timestamp,
) -> Result<UpdateBuilder, StoreError> {
    let target = keys::secret(workspace, name)?;
    Ok(Update::builder()
        .table_name(table)
        .set_key(Some(key(&target.pk, &target.sk)))
        .condition_expression("attribute_exists(pk) AND revocationEpoch = :expectedEpoch")
        .update_expression(
            "SET revocationEpoch = revocationEpoch + :one, \
             revokedThroughRevision = revision, revokedAt = :now, updatedAt = :now",
        )
        .expression_attribute_values(":expectedEpoch", n(expected_epoch.0))
        .expression_attribute_values(":one", n(1))
        .expression_attribute_values(":now", stamp(now)))
}

/// Builds the conditional secret tombstone.
///
/// `aex_secret_domain::delete` bumps the revision to a tombstone, mints no
/// source generation and disturbs no session's custody. This is its one
/// expression, and it is conditional in three ways: the record must exist, must
/// be at the revision the caller read, and must not already be a tombstone. A
/// `DeleteItem` would be wrong here — the row is the fence every use path
/// conditions on, and removing it would make a revoked-then-deleted name read as
/// "never existed" instead of "deleted".
///
/// The sealed generations are deliberately left in place: `delete` is not a
/// revocation, and an in-flight session that already holds custody of an earlier
/// generation must keep working until it is explicitly revoked or rebound.
///
/// # Errors
///
/// [`StoreError`] when the name could not enter a key, or when the observed
/// revision cannot be advanced.
pub fn delete(
    table: &str,
    workspace: WorkspaceId,
    name: &str,
    expected_revision: SecretRevision,
    now: Timestamp,
) -> Result<UpdateBuilder, StoreError> {
    let target = keys::secret(workspace, name)?;
    let next = expected_revision
        .0
        .checked_add(1)
        .ok_or_else(|| StoreError::Invalid {
            detail: "the observed secret revision cannot be advanced".to_owned(),
        })?;
    Ok(Update::builder()
        .table_name(table)
        .set_key(Some(key(&target.pk, &target.sk)))
        .condition_expression(
            "attribute_exists(pk) AND revision = :expectedRevision AND #state <> :deleted",
        )
        .update_expression(
            "SET #state = :deleted, revision = :nextRevision, \
             activeSourceGeneration = :none, updatedAt = :now",
        )
        .expression_attribute_names("#state", "state")
        .expression_attribute_values(":deleted", s(secret_state_str(SecretState::Deleted)))
        .expression_attribute_values(":expectedRevision", n(expected_revision.0))
        .expression_attribute_values(":nextRevision", n(next))
        // A tombstone points at no generation. Leaving the pointer would let a
        // later reader resolve a name the customer believes is gone.
        .expression_attribute_values(":none", n(0))
        .expression_attribute_values(":now", stamp(now)))
}

/// Builds the provider-credential revocation.
///
/// One conditional update on one item, exactly like the secret revoke above and
/// for the same reason: revocation is terminal and monotone.
///
/// It needs no idempotency receipt even though the route declares an
/// `Idempotency-Key`. The scope subject is the credential id and the request has
/// no body, so one scope plus one key can only ever carry one intent — an
/// `idempotency_conflict` is unreachable rather than undetected. A replay
/// observes `revoked`, does not compile a second plan, and answers from the
/// stored row.
///
/// # Errors
///
/// [`StoreError`] when the binding is not `ready`, when the revision cannot be
/// advanced, or when a key component is unusable.
pub fn revoke_provider_credential(
    table: &str,
    credential: &ProviderCredential,
    now: Timestamp,
) -> Result<UpdateBuilder, StoreError> {
    if credential.state != CredentialState::Ready {
        return Err(StoreError::Invalid {
            detail: "a revocation is compiled only from a ready binding".to_owned(),
        });
    }
    let next = credential
        .revision
        .checked_add(1)
        .ok_or_else(|| StoreError::Invalid {
            detail: "the observed credential revision cannot be advanced".to_owned(),
        })?;
    let target = keys::provider_credential(
        credential.workspace,
        credential.provider.as_str(),
        credential.credential,
    )?;
    Ok(Update::builder()
        .table_name(table)
        .set_key(Some(key(&target.pk, &target.sk)))
        .condition_expression(
            "attribute_exists(pk) AND #state = :ready AND revision = :expectedRevision",
        )
        .update_expression(
            "SET #state = :revoked, revision = :nextRevision,              revokedAt = :now, updatedAt = :now",
        )
        .expression_attribute_names("#state", "state")
        .expression_attribute_values(":ready", s(CredentialState::Ready.as_str()))
        .expression_attribute_values(":revoked", s(CredentialState::Revoked.as_str()))
        .expression_attribute_values(":expectedRevision", n(credential.revision))
        .expression_attribute_values(":nextRevision", n(next))
        .expression_attribute_values(":now", stamp(now)))
}

/// One name a custody admission binds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundName {
    /// The name.
    pub name: String,
    /// The record revision the caller read.
    pub bound_revision: SecretRevision,
    /// The generation it admits.
    pub source_generation: SourceGeneration,
}

/// Compiles the idle-only custody admission or rebind.
///
/// Every selected name is guarded by a `ConditionCheck` that refuses a revoked
/// record, so an admission cannot bind a credential a concurrent revoke has
/// already fenced.
///
/// # Errors
///
/// [`StoreError`] when a key component is unusable or an action could not be
/// built.
#[allow(
    clippy::too_many_arguments,
    reason = "each argument is a distinct fence the transaction conditions on; \
              collapsing them into one struct would hide which are load-bearing"
)]
pub fn admit_custody(
    table: &str,
    session: SessionId,
    workspace: WorkspaceId,
    names: &[BoundName],
    bindings: Vec<Item>,
    from_revision: CustodyRevision,
    to_revision: CustodyRevision,
    idle_epoch: u64,
    now: Timestamp,
) -> Result<TransactionPlan, StoreError> {
    let mut plan = TransactionPlan::new(token(
        "cus",
        &[&session.to_string(), &to_revision.0.to_string()],
    ));
    for bound in names {
        let target = keys::secret(workspace, &bound.name)?;
        plan.condition_check(
            Participant::SECRET_METADATA,
            ConditionCheck::builder()
                .table_name(table)
                .set_key(Some(key(&target.pk, &target.sk)))
                .condition_expression(
                    "#state = :ready AND revision = :boundRevision \
                     AND revokedThroughRevision < :boundRevision",
                )
                .expression_attribute_names("#state", "state")
                .expression_attribute_values(":ready", s("ready"))
                .expression_attribute_values(":boundRevision", n(bound.bound_revision.0)),
        )?;
    }
    for binding in bindings {
        plan.put(
            Participant::CUSTODY_BINDING,
            Put::builder()
                .table_name(table)
                .set_item(Some(binding))
                .condition_expression(IMMUTABLE),
        )?;
    }
    let head = keys::custody_head(session);
    plan.update(
        Participant::CUSTODY_HEAD,
        Update::builder()
            .table_name(table)
            .set_key(Some(key(&head.pk, &head.sk)))
            .condition_expression(
                "#state = :active AND custodyRevision = :fromRevision AND idleEpoch = :idleEpoch",
            )
            .update_expression("SET custodyRevision = :toRevision, updatedAt = :now")
            .expression_attribute_names("#state", "state")
            .expression_attribute_values(":active", s("active"))
            .expression_attribute_values(":fromRevision", n(from_revision.0))
            .expression_attribute_values(":toRevision", n(to_revision.0))
            .expression_attribute_values(":idleEpoch", n(idle_epoch))
            .expression_attribute_values(":now", stamp(now)),
    )?;
    Ok(plan)
}

/// The participants a managed-call authorization names, in plan order.
pub const AUTHORIZE_ORDER: [Participant; 3] = [
    Participant::SECRET_METADATA,
    Participant::CUSTODY_HEAD,
    Participant::CUSTODY_AUTHORIZATION,
];

/// Compiles the only gate that stands before a decrypt.
///
/// # Errors
///
/// [`StoreError`] when a key component is unusable or an action could not be
/// built.
pub fn authorize_managed_call(
    table: &str,
    authorization: &CallAuthorization,
    bound_source_revision: SecretRevision,
) -> Result<TransactionPlan, StoreError> {
    let mut plan = TransactionPlan::new(token(
        "aut",
        &[
            &authorization.session.to_string(),
            &authorization.authorization_id,
        ],
    ));
    let secret = keys::secret(authorization.workspace, authorization.name.as_str())?;
    plan.condition_check(
        Participant::SECRET_METADATA,
        ConditionCheck::builder()
            .table_name(table)
            .set_key(Some(key(&secret.pk, &secret.sk)))
            .condition_expression(
                "#state = :ready AND revokedThroughRevision < :boundSourceRevision",
            )
            .expression_attribute_names("#state", "state")
            .expression_attribute_values(":ready", s("ready"))
            .expression_attribute_values(":boundSourceRevision", n(bound_source_revision.0)),
    )?;
    let head = keys::custody_head(authorization.session);
    plan.condition_check(
        Participant::CUSTODY_HEAD,
        ConditionCheck::builder()
            .table_name(table)
            .set_key(Some(key(&head.pk, &head.sk)))
            .condition_expression("#state = :active AND custodyRevision = :expectedRevision")
            .expression_attribute_names("#state", "state")
            .expression_attribute_values(":active", s("active"))
            .expression_attribute_values(":expectedRevision", n(authorization.custody_revision.0)),
    )?;
    plan.put(
        Participant::CUSTODY_AUTHORIZATION,
        Put::builder()
            .table_name(table)
            .set_item(Some(codec::encode_authorization(authorization).map_err(
                |error| StoreError::Invalid {
                    detail: error.to_string(),
                },
            )?))
            .condition_expression(IMMUTABLE),
    )?;
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use aex_secret_domain::revocation::RevocationEpoch;
    use aex_wire::ids::{PrefixedId, Uuid7, WorkspaceId};
    use aex_wire::types::Timestamp;

    use super::{revoke, token};

    const TABLE: &str = "dev-eu-west-1-regional-secret-custody";

    fn workspace() -> WorkspaceId {
        WorkspaceId::from_uuid7(Uuid7::compose(1_754_051_696_789, [1; 10]))
    }

    fn now() -> Timestamp {
        Timestamp::parse("2026-08-01T12:34:56.789Z").expect("the pinned spelling")
    }

    #[test]
    fn a_revoke_is_one_update_on_one_item_whatever_the_generation_count_is() {
        let built = revoke(
            TABLE,
            workspace(),
            "openai-key",
            RevocationEpoch::INITIAL,
            now(),
        )
        .expect("builds")
        .build()
        .expect("a complete update");
        let condition = built.condition_expression().expect("conditional");
        assert!(condition.contains("revocationEpoch = :expectedEpoch"));
        let update = built.update_expression();
        assert!(update.contains("revocationEpoch = revocationEpoch + :one"));
        assert!(
            update.contains("revokedThroughRevision = revision"),
            "the fence is the revision every use path compares against: {update}"
        );
    }

    #[test]
    fn a_transport_token_always_fits_the_provider_ceiling() {
        let long = "n".repeat(400);
        assert!(token("sec", &[&long, &long]).len() <= 36);
    }

    #[test]
    fn a_name_that_could_forge_a_key_stops_the_revoke_builder() {
        assert!(revoke(TABLE, workspace(), "a#b", RevocationEpoch::INITIAL, now()).is_err());
    }
}
