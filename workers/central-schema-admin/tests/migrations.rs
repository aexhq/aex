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
    PepperRow, PepperSeeded, RunnerError, SigningKeySeeded, applied_head, apply_grants,
    check_conservation, diff_grants, expect_applied_head, seed_pepper, seed_signing_key,
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
async fn google_only_identity_constraint_advances_without_rewriting_rows() {
    let fixture = Fixture::start().await;
    let migrator = native_migrator();
    let mut connection = fixture.connect().await;
    migrator
        .run_to(20_260_801_001_400, &mut connection)
        .await
        .expect("the predecessor head applies");
    let user_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO identity.user \
           (id, email, status, revision, created_at, updated_at) \
         VALUES ($1, $2, 'active', 1, now(), now())",
    )
    .bind(user_id)
    .bind(format!("{user_id}@example.com"))
    .execute(&mut connection)
    .await
    .expect("user row");
    sqlx::query(
        "INSERT INTO identity.external_identity \
           (id, user_id, provider, provider_account_id, linked_at) \
         VALUES ($1, $2, 'google', $3, now())",
    )
    .bind(uuid::Uuid::new_v4())
    .bind(user_id)
    .bind(user_id.to_string())
    .execute(&mut connection)
    .await
    .expect("google identity");

    migrator
        .run(&mut connection)
        .await
        .expect("the Google-only constraint applies");
    assert_eq!(
        applied_head(&mut connection).await.expect("head"),
        Some(20_260_814_000_300)
    );
    let provider: String =
        sqlx::query_scalar("SELECT provider FROM identity.external_identity WHERE user_id = $1")
            .bind(user_id)
            .fetch_one(&mut connection)
            .await
            .expect("identity remains");
    assert_eq!(provider, "google");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_legacy_github_identity_aborts_the_constraint_transaction_without_data_loss() {
    let fixture = Fixture::start().await;
    let migrator = native_migrator();
    let mut connection = fixture.connect().await;
    migrator
        .run_to(20_260_801_001_400, &mut connection)
        .await
        .expect("the predecessor head applies");
    let user_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO identity.user \
           (id, email, status, revision, created_at, updated_at) \
         VALUES ($1, $2, 'active', 1, now(), now())",
    )
    .bind(user_id)
    .bind(format!("{user_id}@example.com"))
    .execute(&mut connection)
    .await
    .expect("user row");
    sqlx::query(
        "INSERT INTO identity.external_identity \
           (id, user_id, provider, provider_account_id, linked_at) \
         VALUES ($1, $2, 'github', $3, now())",
    )
    .bind(uuid::Uuid::new_v4())
    .bind(user_id)
    .bind(user_id.to_string())
    .execute(&mut connection)
    .await
    .expect("legacy GitHub identity");

    migrator
        .run(&mut connection)
        .await
        .expect_err("the incompatible row fails closed");
    assert_eq!(
        applied_head(&mut connection).await.expect("head"),
        Some(20_260_801_001_400),
        "the rejected migration never advances history"
    );
    let provider: String =
        sqlx::query_scalar("SELECT provider FROM identity.external_identity WHERE user_id = $1")
            .bind(user_id)
            .fetch_one(&mut connection)
            .await
            .expect("legacy identity remains");
    assert_eq!(provider, "github", "the migration never rewrites the row");
    sqlx::query(
        "INSERT INTO identity.external_identity \
           (id, user_id, provider, provider_account_id, linked_at) \
         VALUES ($1, $2, 'github', $3, now())",
    )
    .bind(uuid::Uuid::new_v4())
    .bind(user_id)
    .bind(format!("second-{user_id}"))
    .execute(&mut connection)
    .await
    .expect("the original constraint was restored by transaction rollback");
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

/// After the whole chain applies, the escrow account is the only fenced one.
///
/// The baseline fenced `customer_available` too, and that outranked the
/// credit-exhaustion pause: the deduction that exhausts an account was refused,
/// so the usage was discarded and the account went on running.
/// `20260801001300` drops it. This asserts the *end state* of the chain rather
/// than any one body, which is what a database actually gets.
#[tokio::test(flavor = "multi_thread")]
async fn only_the_escrow_balance_is_fenced_after_the_chain_applies() {
    let fixture = Fixture::start().await;
    let migrator = native_migrator();
    let mut connection = fixture.connect().await;
    migrator
        .run(&mut connection)
        .await
        .expect("the bundle applies");

    let fences: Vec<String> = sqlx::query_scalar(
        "SELECT conname::text FROM pg_constraint \
          WHERE conrelid = 'finance.account_balance'::regclass AND contype = 'c' \
          ORDER BY conname",
    )
    .fetch_all(&mut connection)
    .await
    .expect("the projection's check constraints read");
    assert_eq!(
        fences,
        vec![
            "balance_bound".to_owned(),
            "reserved_balance_never_overdrawn".to_owned()
        ],
        "the available-balance fence is gone and the escrow fence remains"
    );

    // A platform ledger was never covered by the prepaid fence and still is not.
    connection
        .execute(
            "UPDATE finance.account_balance SET balance_microusd = 1 \
              WHERE account_id = '00000000-0000-7000-8000-000000000003'",
        )
        .await
        .expect("a platform revenue account is not fenced by the prepaid CHECK");
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

#[tokio::test(flavor = "multi_thread")]
async fn every_pepper_row_seeds_once_and_an_exact_replay_is_a_no_op() {
    // No migration writes these rows and nothing else in the tree does either,
    // so a plane whose release skipped this step has a `central-api` that
    // refuses to start and an `api_key_create` that cannot mint.
    let fixture = Fixture::start().await;
    let mut connection = fixture.connect().await;
    native_migrator()
        .run(&mut connection)
        .await
        .expect("the bundle applies");

    for (version, row) in (1_i16..).zip(PepperRow::ALL) {
        let reference = format!("secret-version-{version}");
        assert_eq!(
            seed_pepper(&mut connection, row, version, &reference)
                .await
                .expect("a clean table accepts the row"),
            PepperSeeded::Inserted,
            "{row:?}"
        );
        assert_eq!(
            seed_pepper(&mut connection, row, version, &reference)
                .await
                .expect("the exact same arguments are a replay"),
            PepperSeeded::AlreadyExact,
            "{row:?}"
        );
    }

    // Exactly what `ACTIVE_CONTROL_PEPPER` and `ACTIVE_IDENTITY_PEPPER` read.
    for (table, purpose) in [
        ("identity.credential_pepper", "identity"),
        ("identity.credential_pepper", "cursor"),
        ("control.credential_pepper", "api_key"),
        ("control.credential_pepper", "cursor"),
    ] {
        let found: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT count(*) FROM {table} WHERE purpose = $1 AND state = 'active'"
        )))
        .bind(purpose)
        .fetch_one(&mut connection)
        .await
        .expect("the active row reads");
        assert_eq!(found, 1, "{table} has no active `{purpose}` pepper");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn re_pointing_a_seeded_version_at_other_material_is_refused() {
    // Silently accepting it would make every credential fingerprinted under
    // that version unverifiable while the command reported success.
    let fixture = Fixture::start().await;
    let mut connection = fixture.connect().await;
    native_migrator()
        .run(&mut connection)
        .await
        .expect("the bundle applies");
    seed_pepper(&mut connection, PepperRow::ApiKey, 2, "secret-version-a")
        .await
        .expect("the first seed lands");

    let error = seed_pepper(&mut connection, PepperRow::ApiKey, 2, "secret-version-b")
        .await
        .expect_err("a version cannot be re-pointed");
    assert!(matches!(error, RunnerError::PepperConflict(_)), "{error}");

    // A rotation introduces a second live version and retires the first. That
    // is a ceremony, and this command is deliberately not it.
    let error = seed_pepper(&mut connection, PepperRow::ApiKey, 3, "secret-version-c")
        .await
        .expect_err("a second active row for one purpose is not a seed");
    assert!(
        format!("{error}").contains("rotation"),
        "the refusal names what it is not: {error}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_signing_key_seeds_once_and_an_exact_replay_is_a_no_op() {
    let fixture = Fixture::start().await;
    let mut connection = fixture.connect().await;
    native_migrator()
        .run(&mut connection)
        .await
        .expect("the bundle applies");
    let kid = uuid::Uuid::parse_str("00000000-0000-7000-8000-0000000000d1")
        .expect("the fixture kid parses");
    let public_key = [0x5a; 32];
    let secret_ref = "aex/dev/central/assertion-signing-key";

    assert_eq!(
        seed_signing_key(
            &mut connection,
            kid,
            public_key,
            secret_ref,
            1_786_320_000_000,
            1_801_872_000_000,
        )
        .await
        .expect("a clean table accepts the signing key"),
        SigningKeySeeded::Inserted,
    );
    assert_eq!(
        seed_signing_key(
            &mut connection,
            kid,
            public_key,
            secret_ref,
            1_786_320_000_000,
            1_801_872_000_000,
        )
        .await
        .expect("the exact arguments are a replay"),
        SigningKeySeeded::AlreadyExact,
    );

    let stored: (String, Vec<u8>, String, String, i64, i64, Option<String>) = sqlx::query_as(
        "SELECT alg, public_key, secret_ref, state, \
                    (extract(epoch FROM activates_at) * 1000)::bigint, \
                    (extract(epoch FROM retires_at) * 1000)::bigint, retired_at::text \
               FROM control.signing_key WHERE kid = $1",
    )
    .bind(kid)
    .fetch_one(&mut connection)
    .await
    .expect("the seeded key reads");
    assert_eq!(stored.0, "ed25519");
    assert_eq!(stored.1, public_key);
    assert_eq!(stored.2, secret_ref);
    assert_eq!(stored.3, "active");
    assert_eq!(stored.4, 1_786_320_000_000);
    assert_eq!(stored.5, 1_801_872_000_000);
    assert!(stored.6.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_signing_key_seed_refuses_drift_and_a_second_active_key() {
    let fixture = Fixture::start().await;
    let mut connection = fixture.connect().await;
    native_migrator()
        .run(&mut connection)
        .await
        .expect("the bundle applies");
    let first = uuid::Uuid::parse_str("00000000-0000-7000-8000-0000000000d1")
        .expect("the first kid parses");
    seed_signing_key(
        &mut connection,
        first,
        [0x5a; 32],
        "aex/dev/central/assertion-signing-key",
        1_786_320_000_000,
        1_801_872_000_000,
    )
    .await
    .expect("the first seed lands");

    let drift = seed_signing_key(
        &mut connection,
        first,
        [0x6b; 32],
        "aex/dev/central/assertion-signing-key",
        1_786_320_000_000,
        1_801_872_000_000,
    )
    .await
    .expect_err("a kid cannot be re-pointed at another public key");
    assert!(
        matches!(drift, RunnerError::SigningKeyConflict(_)),
        "{drift}"
    );

    let second = seed_signing_key(
        &mut connection,
        uuid::Uuid::parse_str("00000000-0000-7000-8000-0000000000d2")
            .expect("the second kid parses"),
        [0x7c; 32],
        "aex/dev/central/assertion-signing-key",
        1_786_320_000_000,
        1_801_872_000_000,
    )
    .await
    .expect_err("a second active key is a rotation, not a seed");
    assert!(
        matches!(second, RunnerError::SigningKeyConflict(_)),
        "{second}"
    );
    assert!(second.to_string().contains("rotation"));
}
