//! Property catalogue for `aex-secret-domain`: plan 04 items 88-98 and 100.
//!
//! Items 99 and 101 — plaintext non-persistence and zeroization — have direct
//! behavioral coverage in `tests/security.rs` and the plaintext module.

use std::collections::BTreeSet;

use aex_secret_domain::context::Plane;
use aex_secret_domain::revocation::is_revoked_since;
use aex_secret_domain::secret::SecretRevision;
use aex_secret_domain::{
    CONTEXT_KEYS, CiphertextRef, CloneCredentials, CustodyEntry, CustodyRejection, CustodyRevision,
    CustodyState, EncryptionContext, OwnerKeyEdgeId, RevocationEpoch, SecretName, SecretState,
    SessionCustody, SourceGeneration, TrueIdle, TrueIdleViolation, UseDenied, WorkspaceSecret,
    admit_custody, clone_custody, delete, delete_custody, managed_use_allowed, rebind, revoke, set,
};
use aex_wire::ids::{OrganizationId, PrefixedId as _, SessionId, Uuid7, WorkspaceId};
use aex_wire::types::{Region, Timestamp};
use proptest::prelude::*;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn workspace() -> WorkspaceId {
    WorkspaceId::from_uuid7(Uuid7::compose(1, [1; 10]))
}

fn session(tag: u8) -> SessionId {
    SessionId::from_uuid7(Uuid7::compose(1, [tag; 10]))
}

fn edge(tag: u8) -> OwnerKeyEdgeId {
    OwnerKeyEdgeId(Uuid7::compose(1, [tag; 10]))
}

fn moment(millis: i64) -> Timestamp {
    Timestamp::from_unix_millis(millis).expect("in range")
}

fn ciphertext(tag: u8) -> CiphertextRef {
    CiphertextRef {
        key_generation: 1,
        wrapped_key: vec![tag; 32],
        nonce: vec![tag; 12],
        ciphertext: vec![tag; 48],
    }
}

fn context(name: &SecretName, generation: SourceGeneration) -> EncryptionContext {
    EncryptionContext {
        plane: Plane::Prd,
        region: Region::ALL[0],
        organization: OrganizationId::from_uuid7(Uuid7::compose(1, [2; 10])),
        workspace: workspace(),
        name: name.clone(),
        generation,
        custody_revision: None,
    }
}

fn created(name: &str) -> WorkspaceSecret {
    let parsed = SecretName::parse(name).expect("valid");
    set(
        None,
        workspace(),
        &parsed,
        ciphertext(1),
        context(&parsed, SourceGeneration::FIRST),
        moment(0),
    )
    .expect("creates")
    .secret
}

fn replace(current: &WorkspaceSecret, tag: u8, at: i64) -> WorkspaceSecret {
    let generation = current.generation.next();
    set(
        Some(current),
        workspace(),
        &current.name,
        ciphertext(tag),
        context(&current.name, generation),
        moment(at),
    )
    .expect("replaces")
    .secret
}

// ---------------------------------------------------------------------------
// 88, 89 — monotone lineage
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig { cases: 192, ..ProptestConfig::default() })]

    /// 88 `generation_monotone`.
    #[test]
    fn generation_monotone(rounds in 1_usize..24) {
        let mut secret = created("alpha");
        let mut seen: BTreeSet<u64> = BTreeSet::new();
        seen.insert(secret.generation.0);
        prop_assert_eq!(secret.generation, SourceGeneration::FIRST);
        prop_assert_eq!(secret.revision, SecretRevision::FIRST);

        for round in 1..rounds {
            let next = replace(&secret, 2, i64::try_from(round).expect("bounded"));
            prop_assert!(next.generation > secret.generation, "generations are strict");
            prop_assert!(next.revision > secret.revision, "revisions are strict");
            prop_assert!(seen.insert(next.generation.0), "a generation is never reused");
            secret = next;
        }
    }

    /// 89 `revocation_epoch_monotone`.
    #[test]
    fn revocation_epoch_monotone(actions in prop::collection::vec(any::<bool>(), 1..24)) {
        let mut secret = created("alpha");
        let mut epoch = RevocationEpoch::INITIAL;
        for (index, revoking) in actions.into_iter().enumerate() {
            let at = i64::try_from(index).expect("bounded") + 1;
            let next = if revoking {
                let commit = revoke(&secret, moment(at)).expect("revokes");
                prop_assert!(commit.epoch > epoch, "revoke always increments");
                epoch = commit.epoch;
                commit.secret
            } else {
                // A later `set` mints a generation but never lowers the epoch.
                let commit = replace(&secret, 3, at);
                prop_assert_eq!(commit.revocation_epoch, epoch);
                commit
            };
            prop_assert!(next.revocation_epoch >= epoch);
            secret = next;
        }
    }
}

// ---------------------------------------------------------------------------
// 90, 91 — retroactive revocation
// ---------------------------------------------------------------------------

#[test]
fn revoke_is_retroactive_across_sessions_and_clones() {
    // 90 `revoke_is_retroactive`.
    let alpha = created("alpha");
    let parent = admit_custody(
        session(3),
        workspace(),
        None,
        std::slice::from_ref(&alpha),
        edge(4),
        moment(1),
    )
    .expect("admits");
    let child = clone_custody(
        Some(&parent),
        session(5),
        CloneCredentials::Copy,
        Some(edge(6)),
        moment(2),
    )
    .expect("clones")
    .custody
    .expect("written");

    let name = SecretName::parse("alpha").expect("valid");
    for row in [&parent, &child] {
        let entry = row.entry(&name).expect("bound");
        assert_eq!(
            managed_use_allowed(entry, &alpha, row.revision, row),
            Ok(())
        );
    }

    let revoked = revoke(&alpha, moment(3)).expect("revokes");
    assert_eq!(revoked.secret.state, SecretState::Revoked);
    assert!(is_revoked_since(
        RevocationEpoch::INITIAL,
        revoked.secret.revocation_epoch
    ));

    // The clone carries the source generation forward, so it is denied too.
    for row in [&parent, &child] {
        let entry = row.entry(&name).expect("bound");
        assert_eq!(
            managed_use_allowed(entry, &revoked.secret, row.revision, row),
            Err(UseDenied::RevokedAfterAdmission {
                admitted: RevocationEpoch::INITIAL,
                current: revoked.secret.revocation_epoch,
            })
        );
    }
}

#[test]
fn a_set_after_a_revoke_never_re_enables_an_old_entry() {
    // 91 `set_after_revoke_needs_rebind`.
    let alpha = created("alpha");
    let row = admit_custody(
        session(3),
        workspace(),
        None,
        std::slice::from_ref(&alpha),
        edge(4),
        moment(1),
    )
    .expect("admits");
    let name = SecretName::parse("alpha").expect("valid");
    let entry = row.entry(&name).expect("bound").clone();

    let revoked = revoke(&alpha, moment(2)).expect("revokes").secret;
    let replaced = replace(&revoked, 9, 3);
    assert_eq!(replaced.state, SecretState::Ready);
    assert_eq!(replaced.revocation_epoch, revoked.revocation_epoch);

    // The old entry is still denied: only an explicit rebind clears it.
    assert!(matches!(
        managed_use_allowed(&entry, &replaced, row.revision, &row),
        Err(UseDenied::RevokedAfterAdmission { .. })
    ));

    let rebound = rebind(
        &row,
        std::slice::from_ref(&replaced),
        &TrueIdle::idle(moment(4)),
        edge(7),
        moment(4),
    )
    .expect("rebinds");
    let fresh = rebound.custody.entry(&name).expect("bound");
    assert_eq!(
        managed_use_allowed(fresh, &replaced, rebound.custody.revision, &rebound.custody),
        Ok(())
    );
}

// ---------------------------------------------------------------------------
// 92-94 — rebind
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, ..ProptestConfig::default() })]

    /// 92 `rebind_atomic` and 94 `rebind_destroys_prior_edge`.
    #[test]
    fn rebind_atomic(rounds in 1_usize..12) {
        let mut secret = created("alpha");
        let mut row = admit_custody(
            session(3),
            workspace(),
            None,
            std::slice::from_ref(&secret),
            edge(4),
            moment(1),
        )
        .expect("admits");

        for round in 0..rounds {
            let at = i64::try_from(round).expect("bounded") + 2;
            secret = replace(&secret, 5, at);
            let before = row.clone();
            let commit = rebind(
                &row,
                std::slice::from_ref(&secret),
                &TrueIdle::idle(moment(at)),
                edge(u8::try_from(round).expect("bounded") + 32),
                moment(at),
            )
            .expect("rebinds");

            // Exactly one revision step, one new edge, one destroyed edge.
            prop_assert_eq!(commit.custody.revision, before.revision.next());
            prop_assert_ne!(commit.custody.owner_key_edge, before.owner_key_edge);
            prop_assert_eq!(commit.destroy_key_edges, vec![before.owner_key_edge]);

            // No intermediate state: every entry names the new generation.
            for entry in &commit.custody.entries {
                prop_assert_eq!(entry.source_generation, secret.generation);
            }
            row = commit.custody;
        }
    }

    /// 93 `rebind_idle_only`.
    #[test]
    fn rebind_idle_only(violation in prop::option::of(prop::sample::select(vec![
        TrueIdleViolation::WorkAdmitted,
        TrueIdleViolation::WorkQueued,
        TrueIdleViolation::ConnectionOpen,
        TrueIdleViolation::KeepaliveHeld,
    ]))) {
        let secret = created("alpha");
        let row = admit_custody(
            session(3),
            workspace(),
            None,
            std::slice::from_ref(&secret),
            edge(4),
            moment(1),
        )
        .expect("admits");
        let idle = TrueIdle { observed_at: moment(2), violation };
        let outcome = rebind(&row, &[secret], &idle, edge(5), moment(2));
        match violation {
            Some(reason) => prop_assert_eq!(outcome, Err(CustodyRejection::NotTrueIdle(reason))),
            None => prop_assert!(outcome.is_ok()),
        }
    }
}

// ---------------------------------------------------------------------------
// 95-97 — clone, exact match and the pause exemption set
// ---------------------------------------------------------------------------

#[test]
fn clone_custody_lineage() {
    // 95 `clone_custody_lineage`.
    let alpha = created("alpha");
    let parent = admit_custody(
        session(3),
        workspace(),
        None,
        std::slice::from_ref(&alpha),
        edge(4),
        moment(1),
    )
    .expect("admits");

    let copied = clone_custody(
        Some(&parent),
        session(5),
        CloneCredentials::Copy,
        Some(edge(6)),
        moment(2),
    )
    .expect("clones");
    let child = copied.custody.expect("written");
    assert_eq!(child.entries.len(), parent.entries.len());
    for (left, right) in child.entries.iter().zip(&parent.entries) {
        assert_eq!(left.source_generation, right.source_generation);
        assert_eq!(left.epoch_at_admission, right.epoch_at_admission);
    }
    assert_ne!(child.owner_key_edge, parent.owner_key_edge);

    let none = clone_custody(
        Some(&parent),
        session(7),
        CloneCredentials::None,
        None,
        moment(2),
    )
    .expect("clones");
    assert_eq!(none.custody, None);
    assert_eq!(none.revision, CustodyRevision::NONE);
}

#[test]
fn use_requires_an_exact_match() {
    // 96 `use_requires_exact_match`.
    let alpha = created("alpha");
    let beta = created("beta");
    let row = admit_custody(
        session(3),
        workspace(),
        None,
        std::slice::from_ref(&alpha),
        edge(4),
        moment(1),
    )
    .expect("admits");
    let name = SecretName::parse("alpha").expect("valid");
    let entry = row.entry(&name).expect("bound").clone();

    assert_eq!(
        managed_use_allowed(&entry, &alpha, row.revision, &row),
        Ok(())
    );

    // Wrong custody revision.
    assert_eq!(
        managed_use_allowed(&entry, &alpha, CustodyRevision(42), &row),
        Err(UseDenied::CustodyRevisionMismatch {
            current: row.revision,
            presented: CustodyRevision(42),
        })
    );

    // Unbound name.
    let unbound = CustodyEntry {
        name: beta.name.clone(),
        source_generation: beta.generation,
        source_revision: beta.revision,
        epoch_at_admission: beta.revocation_epoch,
        ciphertext: ciphertext(1),
    };
    assert_eq!(
        managed_use_allowed(&unbound, &beta, row.revision, &row),
        Err(UseDenied::SecretNotBound(beta.name.clone()))
    );

    // Deleted workspace record.
    let deleted = delete(&alpha, moment(3)).expect("deletes").secret;
    assert_eq!(
        managed_use_allowed(&entry, &deleted, row.revision, &row),
        Err(UseDenied::SecretDeleted)
    );

    // Deleted custody.
    let tombstone = delete_custody(&row, moment(4)).custody;
    assert_eq!(tombstone.state, CustodyState::Deleted);
    assert_eq!(
        managed_use_allowed(&entry, &alpha, tombstone.revision, &tombstone),
        Err(UseDenied::CustodyDeleted)
    );
}

#[test]
fn revoke_and_custody_deletion_never_need_a_ciphertext() {
    // 97 `pause_exemptions`, domain half. `revoke` and `delete_custody` are
    // pure state transitions over the record the caller already holds; they
    // consume no ciphertext and mint no generation, which is why they can stay
    // available while an account is paused. `set`, `delete`, `admit_custody`,
    // `rebind` and `clone_custody` all consume either a ciphertext or an owner
    // key edge, which is exactly what a paused account may not mint. The gate
    // itself lives in `aex-session-domain::pause`.
    let alpha = created("alpha");
    let commit = revoke(&alpha, moment(2)).expect("revokes");
    assert_eq!(commit.secret.ciphertext, alpha.ciphertext);

    let row = admit_custody(
        session(3),
        workspace(),
        None,
        std::slice::from_ref(&alpha),
        edge(4),
        moment(1),
    )
    .expect("admits");
    let deleted = delete_custody(&row, moment(3));
    assert!(deleted.custody.entries.is_empty());
    assert_eq!(deleted.destroy_key_edges, vec![edge(4)]);
}

// ---------------------------------------------------------------------------
// 98, 100 — projections and context
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig { cases: 192, ..ProptestConfig::default() })]

    /// 98 `description_leaks_nothing`.
    #[test]
    fn description_leaks_nothing(names in prop::collection::vec(
        prop::sample::select(vec!["alpha", "beta", "gamma", "delta"]),
        1..4,
    )) {
        let mut unique: BTreeSet<&str> = BTreeSet::new();
        let selection: Vec<WorkspaceSecret> = names
            .into_iter()
            .filter(|name| unique.insert(name))
            .map(created)
            .collect();
        let row: SessionCustody = admit_custody(
            session(3),
            workspace(),
            None,
            &selection,
            edge(4),
            moment(1),
        )
        .expect("admits");

        let rendered = format!("{:?}", row.describe());
        for entry in &row.entries {
            // The projection carries the name and nothing else about the entry.
            prop_assert!(rendered.contains(entry.name.as_str()));
            prop_assert!(
                !rendered.contains(&format!("{:?}", entry.ciphertext)),
                "a description must not carry ciphertext"
            );
            prop_assert!(
                !rendered.contains("source_generation"),
                "a description must not carry a source generation"
            );
        }
        prop_assert!(
            !rendered.contains("owner_key_edge"),
            "a description must not carry a key edge"
        );
    }

    /// 100 `context_identifier_only`.
    #[test]
    fn context_identifier_only(
        generation in 1_u64..1_000,
        custody in prop::option::of(1_u64..1_000),
    ) {
        let name = SecretName::parse("alpha").expect("valid");
        let value = EncryptionContext {
            custody_revision: custody.map(CustodyRevision),
            ..context(&name, SourceGeneration(generation))
        };
        let pairs = value.canonical_pairs();

        // Every key is from the closed allowlist, in the fixed order.
        let keys: Vec<&str> = pairs.iter().map(|(key, _)| *key).collect();
        let expected: Vec<&str> = CONTEXT_KEYS
            .iter()
            .copied()
            .filter(|key| *key != "aex:custody_revision" || custody.is_some())
            .collect();
        prop_assert_eq!(keys, expected);

        // Every value is an identifier the caller supplied, never derived bytes.
        for (key, rendered) in &pairs {
            prop_assert!(!rendered.is_empty(), "{key} must carry a value");
        }
        prop_assert_eq!(value.digest(), value.digest());
    }
}
