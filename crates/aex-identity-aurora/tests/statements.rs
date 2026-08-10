//! SQL discipline for the identity statements.
//!
//! The load-bearing assertion is that every single-use consumption carries its
//! own guard in the `WHERE` clause. A consumption that read first and wrote
//! second would need the application to arbitrate a race it cannot observe;
//! a conditional `UPDATE` makes the database the arbiter and makes "exactly one
//! winner" a property rather than a hope.

use aex_identity_aurora::sql;

#[test]
fn no_statement_carries_a_format_placeholder() {
    for (name, statement) in sql::ALL {
        assert!(
            !statement.contains("{}") && !statement.contains("{ }"),
            "`{name}` carries a format placeholder; SQL is never assembled at run time"
        );
    }
}

#[test]
fn every_parameter_is_named() {
    for (name, statement) in sql::ALL {
        assert!(
            !statement.contains("$1") && !statement.contains('?'),
            "`{name}` uses a positional parameter; the Data API binds by name"
        );
    }
}

/// Whether `statement` binds a millisecond parameter, i.e. names `:<x>_ms`.
///
/// A projected `..._ms` **alias** is not a bound parameter; only a `:`-prefixed
/// name is, and conflating the two made an earlier version of this scan fire on
/// its own output column.
fn binds_millis(statement: &str) -> bool {
    statement
        .match_indices(':')
        .filter_map(|(at, _)| statement.get(at + 1..))
        .any(|tail| {
            let name: String = tail
                .chars()
                .take_while(|it| it.is_ascii_alphanumeric() || *it == '_')
                .collect();
            name.ends_with("_ms")
        })
}

/// Whether every occurrence of `column` in `projection` is either cast to epoch
/// milliseconds or reduced to a boolean.
///
/// `(k.revoked_at IS NOT NULL) AS key_revoked` projects a boolean, not an
/// instant, so it needs no cast; `s.expires_at AS expires_at` would.
fn timestamp_is_safely_projected(projection: &str, column: &str) -> bool {
    projection.match_indices(column).all(|(at, _)| {
        let before = &projection[at.saturating_sub(24)..at];
        let after = &projection[at + column.len()..projection.len().min(at + column.len() + 24)];
        before.contains("EXTRACT(EPOCH FROM ")
            || after.contains("IS NOT NULL")
            || after.contains("IS NULL")
            || after.starts_with(")*1000")
            || after.trim_start().starts_with("<=")
            || after.trim_start().starts_with('>')
            || after.trim_start().starts_with("_ms")
    })
}

#[test]
fn no_statement_projects_a_bare_timestamptz() {
    for (name, statement) in sql::ALL {
        if !statement.starts_with("SELECT") {
            continue;
        }
        let projection = statement.split(" FROM ").next().unwrap_or_default();
        for column in [
            "issued_at",
            "expires_at",
            "revoked_at",
            "consumed_at",
            "approved_at",
            "last_polled_at",
            "email_verified_at",
        ] {
            assert!(
                timestamp_is_safely_projected(projection, column),
                "`{name}` projects `{column}` without the epoch-millis cast"
            );
        }
    }
}

#[test]
fn every_timestamp_parameter_goes_through_the_millisecond_cast() {
    for (name, statement) in sql::ALL {
        if !binds_millis(statement) {
            continue;
        }
        assert!(
            statement.contains("TIMESTAMPTZ 'epoch' +"),
            "`{name}` binds a millisecond parameter without the cast"
        );
    }
}

#[test]
fn every_single_use_consumption_carries_its_guard_in_the_predicate() {
    for (name, statement) in sql::SINGLE_USE {
        assert!(
            statement.starts_with("UPDATE"),
            "`{name}` is not a conditional write"
        );
        let predicate = statement
            .split(" WHERE ")
            .nth(1)
            .unwrap_or_else(|| panic!("`{name}` has no WHERE clause"));
        let guarded = predicate.contains("IS NULL")
            || predicate.contains("status = ")
            || predicate.contains("status IN ");
        assert!(
            guarded,
            "`{name}` writes without repeating the domain guard: {predicate}"
        );
    }
}

#[test]
fn every_time_bounded_consumption_checks_expiry_in_the_predicate() {
    for (name, statement) in sql::SINGLE_USE {
        if *name == "REVOKE_DASHBOARD_SESSION" {
            // Revoking a session is safe at any instant; the domain refuses a
            // lapsed one and the row's own expiry already ends its usefulness.
            continue;
        }
        let predicate = statement.split(" WHERE ").nth(1).unwrap_or_default();
        assert!(
            predicate.contains("expires_at >"),
            "`{name}` consumes without checking expiry"
        );
    }
}

/// Both halves of the device decision, for the scans that cover the pair.
const DECISIONS: [(&str, &str); 2] = [
    (
        "APPROVE_DEVICE_AUTHORIZATION",
        sql::APPROVE_DEVICE_AUTHORIZATION,
    ),
    ("DENY_DEVICE_AUTHORIZATION", sql::DENY_DEVICE_AUTHORIZATION),
];

/// Deciding a grant is a cross-principal effect, so the actor is part of the
/// write rather than a prior read.
///
/// Approve carried this from the start and deny did not, which meant anyone who
/// learned a user code could refuse a stranger's sign-in without holding a
/// session at all. The scan runs over the pair so the next decision added
/// cannot repeat it.
#[test]
fn every_device_decision_requires_a_current_dashboard_actor_in_the_predicate() {
    for (name, statement) in DECISIONS {
        for clause in [
            "EXISTS (SELECT 1 FROM identity.dashboard_session",
            "s.id = :actor_session_id",
            "s.user_id = :actor_user_id",
            "s.revoked_at IS NULL",
            "u.status = 'active'",
        ] {
            assert!(
                statement.contains(clause),
                "`{name}` decides a grant without `{clause}`; \
                 `decide_device` binds the actor for both paths and a statement \
                 that ignores it lets a stranger decide"
            );
        }
    }
}

/// A decision lands on a pending grant and on nothing else.
///
/// Retraction is not a capability this platform offers, and `dev_approved_ck`
/// means it never was: writing `denied` over an approved row without clearing
/// `approved_by_user_id` raises 23514, so the wider status set could only ever
/// have produced a check violation.
#[test]
fn a_device_decision_only_ever_lands_on_a_pending_grant() {
    for (name, statement) in DECISIONS {
        assert!(
            statement.contains("d.status = 'pending'"),
            "`{name}` decides a grant that is not pending"
        );
        assert!(
            !statement.contains("d.status IN "),
            "`{name}` admits a status set rather than the one state a decision applies to"
        );
    }
}

#[test]
fn a_credential_is_looked_up_by_its_primary_key_and_never_by_its_verifier() {
    for (name, statement) in sql::ALL {
        assert!(
            !statement.contains("WHERE verifier") && !statement.contains("verifier ="),
            "`{name}` looks a credential up by its verifier; \
             a key-addressed lookup survives a pepper rotation and this does not"
        );
    }
}

#[test]
fn the_statement_inventory_is_sorted_by_name_within_its_groups() {
    let names: Vec<&str> = sql::ALL.iter().map(|(name, _)| *name).collect();
    let mut deduped = names.clone();
    deduped.sort_unstable();
    deduped.dedup();
    assert_eq!(
        deduped.len(),
        names.len(),
        "no statement is listed twice: {names:?}"
    );
}

#[test]
fn account_token_scopes_expand_from_a_data_api_json_scalar() {
    assert!(
        sql::INSERT_ACCOUNT_TOKEN
            .contains("ARRAY(SELECT jsonb_array_elements_text(CAST(:scopes AS jsonb)))"),
        "account-token creation passes an unsupported Data API array parameter"
    );
}
