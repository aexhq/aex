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
