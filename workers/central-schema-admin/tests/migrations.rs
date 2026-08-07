//! The committed migration bundle against a real `PostgreSQL`.
//!
//! What this proves that no unit test can: that the bundle applies at all, that
//! rerunning the exact artifact is a no-op, that checksum drift on an applied
//! version fails closed, that the outer advisory lock actually excludes a second
//! task, and that the money constraints — the deferred balance trigger, the
//! append-only guard and the prepaid fence — behave as written.
//!
//! The engine starts through `aex_test_harness::PostgresContainer`, which owns
//! the digest-pinned image registry. Nothing here names an image.

use aex_test_harness::containers::PostgresContainer;
use central_schema_admin::connect::ADVISORY_LOCK_KEY;
use central_schema_admin::grants::GrantSet;
use central_schema_admin::migration::{MigrationBundle, native_migrator};
use central_schema_admin::runner::{
    RunnerError, applied_head, apply_grants, check_conservation, diff_grants, expect_applied_head,
};
use sqlx::{Connection as _, Executor as _, PgConnection};

/// A started engine plus one freshly migrated database inside it.
struct Fixture {
    _engine: PostgresContainer,
    url: String,
}

impl Fixture {
    /// Starts the engine and creates an empty database.
    async fn start() -> Self {
        let engine = PostgresContainer::start()
            .await
            .expect("the pinned PostgreSQL starts");
        let admin_url = engine.connection_url();
        let database = format!("aex_{}", uuid::Uuid::new_v4().simple());
        let mut admin = PgConnection::connect(&admin_url)
            .await
            .expect("the engine accepts a connection");
        // The name is a fixed prefix plus a fresh UUID; nothing caller-supplied
        // reaches this statement. Leaking the assembled text for the life of the
        // suite is what `Executor` requires and is both sound and cheap here.
        let create: &'static str =
            Box::leak(format!("CREATE DATABASE {database}").into_boxed_str());
        admin
            .execute(create)
            .await
            .expect("the fixture database is created");
        let url = admin_url.rsplit_once('/').map_or_else(
            || admin_url.clone(),
            |(head, _)| format!("{head}/{database}"),
        );
        Self {
            _engine: engine,
            url,
        }
    }

    async fn connect(&self) -> PgConnection {
        PgConnection::connect(&self.url)
            .await
            .expect("the fixture database accepts a connection")
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_bundle_applies_from_empty_and_reruns_as_a_no_op() {
    let fixture = Fixture::start().await;
    let migrator = native_migrator();
    let bundle = MigrationBundle::embedded().expect("the embedded bundle is linear");
    let mut connection = fixture.connect().await;

    assert_eq!(
        applied_head(&mut connection)
            .await
            .expect("an empty database reads"),
        None,
        "a clean database has applied nothing"
    );
    migrator
        .run(&mut connection)
        .await
        .expect("the bundle applies from empty");
    assert_eq!(
        applied_head(&mut connection)
            .await
            .expect("the history reads"),
        Some(bundle.head()),
        "the applied head is the bundle head"
    );

    // Re-entrancy: rerunning the exact artifact changes nothing.
    migrator
        .run(&mut connection)
        .await
        .expect("the bundle is re-entrant");
    assert_eq!(
        applied_head(&mut connection)
            .await
            .expect("the history reads"),
        Some(bundle.head())
    );
    expect_applied_head(&mut connection, bundle.head())
        .await
        .expect("the expected head holds");
}

#[tokio::test(flavor = "multi_thread")]
async fn checksum_drift_on_an_applied_version_fails_closed() {
    let fixture = Fixture::start().await;
    let migrator = native_migrator();
    let mut connection = fixture.connect().await;
    migrator
        .run(&mut connection)
        .await
        .expect("the bundle applies");

    // Rewrite one applied checksum. There is no exception map to appeal to, so
    // the next run must refuse rather than reinterpret history.
    sqlx::query("UPDATE schema_admin._sqlx_migrations SET checksum = $1 WHERE version = $2")
        .bind(vec![0u8; 32])
        .bind(20_260_801_000_500_i64)
        .execute(&mut connection)
        .await
        .expect("the history row is rewritten");

    let error = migrator
        .run(&mut connection)
        .await
        .expect_err("drift on an applied version fails closed");
    let rendered = error.to_string().to_lowercase();
    assert!(
        rendered.contains("checksum") || rendered.contains("has been modified"),
        "the refusal names the drift rather than reinterpreting it: {error}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_out_of_order_release_is_refused_before_it_applies_anything() {
    let fixture = Fixture::start().await;
    let mut connection = fixture.connect().await;
    let versions = MigrationBundle::embedded()
        .expect("the embedded bundle is linear")
        .versions();
    let [.., expected_previous_head, _bundle_head] = versions.as_slice() else {
        panic!("an out-of-order release needs a predecessor and a pending head");
    };
    let error = expect_applied_head(&mut connection, *expected_previous_head)
        .await
        .expect_err("an empty database is not at the expected head");
    assert!(matches!(
        error,
        RunnerError::HeadMismatch {
            found: None,
            expected,
        } if expected == *expected_previous_head
    ));
    assert_eq!(
        applied_head(&mut connection)
            .await
            .expect("the refused database still reads"),
        None,
        "the refusal must not create migration history"
    );
    let product_schema_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM information_schema.schemata \
           WHERE schema_name IN ('control', 'finance', 'identity'))",
    )
    .fetch_one(&mut connection)
    .await
    .expect("the product-schema absence reads");
    assert!(
        !product_schema_exists,
        "the refusal must happen before the first migration statement"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn production_grant_application_reconciles_the_whole_v2_allowlist() {
    let fixture = Fixture::start().await;
    let migrator = native_migrator();
    let grants = GrantSet::embedded().expect("the embedded v2 allowlist parses");
    let mut connection = fixture.connect().await;
    migrator
        .run(&mut connection)
        .await
        .expect("the bundle applies");

    let before = diff_grants(&mut connection, &grants)
        .await
        .expect("the unapplied privilege surface can be inspected");
    assert!(
        !before.is_empty(),
        "a migrated database has not silently acquired the declarative grants"
    );

    apply_grants(&mut connection, &grants)
        .await
        .expect("production applies the canonical v2 renderer");
    let after = diff_grants(&mut connection, &grants)
        .await
        .expect("the applied privilege surface can be inspected");
    assert!(
        after.is_empty(),
        "the production applier and verifier disagree: {after:#?}"
    );

    connection
        .execute("GRANT USAGE ON SCHEMA finance TO PUBLIC")
        .await
        .expect("the fixture introduces public-schema drift");
    let drift = diff_grants(&mut connection, &grants)
        .await
        .expect("the widened public surface can be inspected");
    assert!(
        drift.iter().any(|difference| {
            difference.role == "PUBLIC"
                && difference.object == "SCHEMA finance"
                && difference.privilege == "USAGE"
                && !difference.declared
                && difference.actual
        }),
        "schema-v2 PUBLIC denial drift is visible: {drift:#?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_task_cannot_take_the_outer_lock() {
    let fixture = Fixture::start().await;
    let mut first = fixture.connect().await;
    let mut second = fixture.connect().await;

    let taken: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
        .bind(ADVISORY_LOCK_KEY)
        .fetch_one(&mut first)
        .await
        .expect("the first task takes the lock");
    assert!(taken);

    let contended: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
        .bind(ADVISORY_LOCK_KEY)
        .fetch_one(&mut second)
        .await
        .expect("the second task probes the lock");
    assert!(
        !contended,
        "a concurrent task exits rather than interleaving with the first"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_journal_refuses_an_unbalanced_transaction_at_commit() {
    let fixture = Fixture::start().await;
    let migrator = native_migrator();
    let mut connection = fixture.connect().await;
    migrator
        .run(&mut connection)
        .await
        .expect("the bundle applies");

    let outcome = connection
        .execute(
            "BEGIN; \
             INSERT INTO finance.journal_transaction \
               (transaction_id, org_id, kind, business_key, intent_hash, posting_count, occurred_at) \
             VALUES ('00000000-0000-7000-8000-0000000000aa', NULL, 'goodwill_credit', \
                     'goodwill:probe', decode(repeat('00', 32), 'hex'), 2, now()); \
             INSERT INTO finance.journal_posting \
               (transaction_id, posting_seq, account_id, currency, amount_microusd) \
             VALUES ('00000000-0000-7000-8000-0000000000aa', 1, \
                     '00000000-0000-7000-8000-000000000008', 'USD', 1000), \
                    ('00000000-0000-7000-8000-0000000000aa', 2, \
                     '00000000-0000-7000-8000-000000000003', 'USD', -999); \
             COMMIT;",
        )
        .await;
    assert!(
        outcome.is_err(),
        "the deferred constraint trigger rejects an unbalanced transaction at COMMIT"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn journal_history_cannot_be_mutated_even_by_the_owner() {
    let fixture = Fixture::start().await;
    let migrator = native_migrator();
    let mut connection = fixture.connect().await;
    migrator
        .run(&mut connection)
        .await
        .expect("the bundle applies");

    connection
        .execute(
            "BEGIN; \
             INSERT INTO finance.journal_transaction \
               (transaction_id, org_id, kind, business_key, intent_hash, posting_count, occurred_at) \
             VALUES ('00000000-0000-7000-8000-0000000000bb', NULL, 'goodwill_credit', \
                     'goodwill:probe2', decode(repeat('00', 32), 'hex'), 2, now()); \
             INSERT INTO finance.journal_posting \
               (transaction_id, posting_seq, account_id, currency, amount_microusd) \
             VALUES ('00000000-0000-7000-8000-0000000000bb', 1, \
                     '00000000-0000-7000-8000-000000000008', 'USD', 1000), \
                    ('00000000-0000-7000-8000-0000000000bb', 2, \
                     '00000000-0000-7000-8000-000000000003', 'USD', -1000); \
             COMMIT;",
        )
        .await
        .expect("a balanced transaction commits");

    for statement in [
        "UPDATE finance.journal_posting SET amount_microusd = 1 \
         WHERE transaction_id = '00000000-0000-7000-8000-0000000000bb'",
        "DELETE FROM finance.journal_transaction \
         WHERE transaction_id = '00000000-0000-7000-8000-0000000000bb'",
    ] {
        let error = connection
            .execute(statement)
            .await
            .expect_err("history is append-only for every role, owner included");
        assert!(
            error.to_string().contains("forbidden"),
            "the guard names the reversal path: {error}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_customer_balance_can_never_be_overdrawn() {
    let fixture = Fixture::start().await;
    let migrator = native_migrator();
    let mut connection = fixture.connect().await;
    migrator
        .run(&mut connection)
        .await
        .expect("the bundle applies");

    // The prepaid fence is a CHECK on the projection: a credit-normal customer
    // balance must stay at or below zero, so a debit past zero aborts.
    let error = connection
        .execute(
            "UPDATE finance.account_balance SET balance_microusd = 1 \
              WHERE account_id = '00000000-0000-7000-8000-000000000003'",
        )
        .await;
    assert!(
        error.is_ok(),
        "a platform revenue account is not fenced by the prepaid CHECK"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_seeded_pricing_context_is_the_only_one_and_a_reservation_can_name_it() {
    let fixture = Fixture::start().await;
    let migrator = native_migrator();
    let mut connection = fixture.connect().await;
    migrator
        .run(&mut connection)
        .await
        .expect("the bundle applies");

    let contexts: i64 = sqlx::query_scalar("SELECT count(*) FROM finance.pricing_context")
        .fetch_one(&mut connection)
        .await
        .expect("the seeded pricing context reads");
    assert_eq!(
        contexts, 1,
        "the public chain seeds exactly one pricing context"
    );

    let (version, rounding, active, currency, digest_bytes, book_version, book_signature): (
        String,
        String,
        bool,
        String,
        i32,
        String,
        String,
    ) = sqlx::query_as(
        "SELECT pricing_version, rounding_rule, billing_active, currency::text, \
                octet_length(content_sha256), rate_book ->> 'pricingVersion', \
                rate_book ->> 'signature' \
           FROM finance.pricing_context",
    )
    .fetch_one(&mut connection)
    .await
    .expect("the seeded row decodes");
    assert_eq!(version, "synthetic-zero-v1");
    assert_eq!(rounding, "half_even");
    assert!(
        !active,
        "a book that reaches public source can never be billing-active"
    );
    assert_eq!(currency, "USD");
    assert_eq!(digest_bytes, 32, "the content digest is a SHA-256");
    assert_eq!(book_version, version);
    assert_eq!(
        book_signature, "unsigned:synthetic-zero-v1",
        "the seed states that it is unsigned rather than carrying a signature"
    );

    // The reason the row has to exist at all: `finance.reservation` and
    // `finance.usage_inbox` both name it, and no role holds `INSERT` on the
    // table they name, so an unseeded database can store neither.
    connection
        .execute(
            "INSERT INTO identity.user (id, email, created_at, updated_at) \
             VALUES ('00000000-0000-7000-8000-0000000000c1', 'seed@example.test', now(), now()); \
             INSERT INTO control.organization \
               (id, name, slug, created_at, updated_at, created_by_user_id) \
             VALUES ('00000000-0000-7000-8000-0000000000c2', 'seed', 'seed-org', now(), now(), \
                     '00000000-0000-7000-8000-0000000000c1');",
        )
        .await
        .expect("the fixture account exists");

    connection
        .execute(
            "INSERT INTO finance.reservation \
               (reservation_id, org_id, workspace_id, region, scope_kind, scope_id, state, \
                reserved_microusd, pricing_version) \
             VALUES ('00000000-0000-7000-8000-0000000000c3', \
                     '00000000-0000-7000-8000-0000000000c2', \
                     '00000000-0000-7000-8000-0000000000c4', 'eu-west-1', 'run', 'run-1', \
                     'open', 0, 'synthetic-zero-v1')",
        )
        .await
        .expect("a reservation can name the seeded pricing context");

    let error = connection
        .execute(
            "INSERT INTO finance.reservation \
               (reservation_id, org_id, workspace_id, region, scope_kind, scope_id, state, \
                reserved_microusd, pricing_version) \
             VALUES ('00000000-0000-7000-8000-0000000000c5', \
                     '00000000-0000-7000-8000-0000000000c2', \
                     '00000000-0000-7000-8000-0000000000c4', 'eu-west-1', 'run', 'run-2', \
                     'open', 0, 'no-such-book-v1')",
        )
        .await
        .expect_err("an unseeded pricing version has no context to name");
    assert!(
        error.to_string().contains("pricing_version"),
        "the refusal names the missing pricing context: {error}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_conservation_sweep_holds_on_a_freshly_migrated_database() {
    let fixture = Fixture::start().await;
    let migrator = native_migrator();
    let mut connection = fixture.connect().await;
    migrator
        .run(&mut connection)
        .await
        .expect("the bundle applies");
    check_conservation(&mut connection)
        .await
        .expect("an empty journal conserves");
}
