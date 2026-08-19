//! SQLite persistence: accounts, key custody (hashes only), the prepaid ledger, session
//! ownership and meter state.
//!
//! Money semantics: the ledger holds one signed row per `ref` in micro-USD.
//! `topup:<id>` rows are credits, inserted exactly once (idempotent by primary key).
//! `usage:<session>` rows are debits, OVERWRITTEN by each sweep with the absolute rated total
//! for that session — never incremented — so a replayed or racing sweep cannot double-bill.
//! Balance = SUM(ledger). Meter-state updates are fenced monotonic (`folded_seq`,
//! `metered_to_ms` never move backwards), so concurrent sweeps are safe: the loser's write is
//! simply skipped.

use std::path::Path;
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, OptionalExtension, params};

use crate::rating::FoldState;
use crate::{Error, Result};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS accounts (
  id TEXT PRIMARY KEY,
  email TEXT NOT NULL UNIQUE,
  token_hash TEXT NOT NULL UNIQUE,
  created_ms INTEGER NOT NULL,
  max_concurrent_sessions INTEGER NOT NULL,
  session_creates_per_hour INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS api_keys (
  id TEXT PRIMARY KEY,
  account_id TEXT NOT NULL REFERENCES accounts(id),
  name TEXT NOT NULL,
  prefix TEXT NOT NULL,
  secret_hash TEXT NOT NULL UNIQUE,
  created_ms INTEGER NOT NULL,
  last_used_ms INTEGER,
  revoked_ms INTEGER
);
CREATE INDEX IF NOT EXISTS api_keys_account ON api_keys(account_id);
CREATE TABLE IF NOT EXISTS topups (
  id TEXT PRIMARY KEY,
  account_id TEXT NOT NULL REFERENCES accounts(id),
  amount_cents INTEGER NOT NULL,
  status TEXT NOT NULL,
  provider TEXT NOT NULL,
  provider_ref TEXT NOT NULL,
  checkout_url TEXT,
  created_ms INTEGER NOT NULL,
  paid_ms INTEGER
);
CREATE INDEX IF NOT EXISTS topups_account ON topups(account_id);
CREATE TABLE IF NOT EXISTS ledger (
  ref TEXT PRIMARY KEY,
  account_id TEXT NOT NULL REFERENCES accounts(id),
  microusd INTEGER NOT NULL,
  updated_ms INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS ledger_account ON ledger(account_id);
CREATE TABLE IF NOT EXISTS sessions (
  id TEXT PRIMARY KEY,
  account_id TEXT NOT NULL REFERENCES accounts(id),
  key_id TEXT NOT NULL,
  shape TEXT NOT NULL,
  created_ms INTEGER NOT NULL,
  final INTEGER NOT NULL DEFAULT 0,
  folded_seq INTEGER NOT NULL DEFAULT 0,
  running_ms INTEGER NOT NULL DEFAULT 0,
  turn_open_ms INTEGER,
  susp_byte_s INTEGER NOT NULL DEFAULT 0,
  ws_byte_s INTEGER NOT NULL DEFAULT 0,
  art_byte_s INTEGER NOT NULL DEFAULT 0,
  web_search_queries INTEGER NOT NULL DEFAULT 0,
  metered_to_ms INTEGER NOT NULL DEFAULT 0,
  workspace_bytes INTEGER NOT NULL DEFAULT 0,
  suspended_bytes INTEGER NOT NULL DEFAULT 0,
  artifact_bytes INTEGER NOT NULL DEFAULT 0,
  hand_state TEXT NOT NULL DEFAULT 'preparing',
  session_state TEXT NOT NULL DEFAULT 'active'
);
CREATE INDEX IF NOT EXISTS sessions_account ON sessions(account_id);
";

#[derive(Debug, Clone)]
pub struct AccountRow {
    pub id: String,
    pub email: String,
    pub created_ms: i64,
    pub max_concurrent_sessions: i64,
    pub session_creates_per_hour: i64,
}

#[derive(Debug, Clone)]
pub struct KeyRow {
    pub id: String,
    pub account_id: String,
    pub name: String,
    pub prefix: String,
    pub created_ms: i64,
    pub last_used_ms: Option<i64>,
    pub revoked_ms: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct TopupRow {
    pub id: String,
    pub account_id: String,
    pub amount_cents: i64,
    pub status: String,
    pub provider: String,
    pub provider_ref: String,
    pub checkout_url: Option<String>,
    pub created_ms: i64,
    pub paid_ms: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct SessionRow {
    pub id: String,
    pub account_id: String,
    pub key_id: String,
    pub shape: String,
    pub created_ms: i64,
    pub is_final: bool,
    pub fold: FoldState,
}

/// One SQLite file behind a mutex; every call runs on the blocking pool. Control-plane QPS is
/// tiny; correctness (real transactions for money) beats cleverness here.
#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent()
            && !dir.as_os_str().is_empty()
        {
            std::fs::create_dir_all(dir).map_err(|e| Error::Internal(format!("mkdir: {e}")))?;
        }
        let conn = Connection::open(path).map_err(internal)?;
        Self::init(conn)
    }

    pub fn open_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory().map_err(internal)?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL").ok();
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(internal)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(internal)?;
        conn.execute_batch(SCHEMA).map_err(internal)?;
        // EFS carries the SQLite ledger across task and image upgrades. Additive schema
        // changes therefore migrate in place; new databases already contain the column.
        let has_web_search_queries = {
            let mut stmt = conn
                .prepare("PRAGMA table_info(sessions)")
                .map_err(internal)?;
            let names = stmt
                .query_map([], |r| r.get::<_, String>(1))
                .map_err(internal)?;
            names
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(internal)?
                .iter()
                .any(|name| name == "web_search_queries")
        };
        if !has_web_search_queries {
            conn.execute(
                "ALTER TABLE sessions ADD COLUMN web_search_queries INTEGER NOT NULL DEFAULT 0",
                [],
            )
            .map_err(internal)?;
        }
        Ok(Db {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Run `f` with the connection on the blocking pool.
    pub async fn call<T, F>(&self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> rusqlite::Result<T> + Send + 'static,
    {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let mut guard = conn.lock().expect("db mutex poisoned");
            f(&mut guard)
        })
        .await
        .map_err(|e| Error::Internal(format!("db task: {e}")))?
        .map_err(internal)
    }

    // ---- identity ----

    pub async fn create_account(
        &self,
        row: AccountRow,
        email: String,
        token_hash: String,
    ) -> Result<()> {
        let r = self
            .call(move |c| {
                c.execute(
                    "INSERT INTO accounts (id, email, token_hash, created_ms, max_concurrent_sessions, session_creates_per_hour)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![row.id, email, token_hash, row.created_ms, row.max_concurrent_sessions, row.session_creates_per_hour],
                )
            })
            .await;
        match r {
            Ok(_) => Ok(()),
            Err(Error::Internal(m)) if m.contains("UNIQUE") => Err(Error::Conflict(
                "an account with this email already exists".into(),
            )),
            Err(e) => Err(e),
        }
    }

    pub async fn account_by_token_hash(&self, token_hash: String) -> Result<Option<AccountRow>> {
        self.call(move |c| {
            c.query_row(
                "SELECT id, email, created_ms, max_concurrent_sessions, session_creates_per_hour
                 FROM accounts WHERE token_hash = ?1",
                params![token_hash],
                account_row,
            )
            .optional()
        })
        .await
    }

    pub async fn create_key(&self, row: KeyRow, secret_hash: String) -> Result<()> {
        self.call(move |c| {
            c.execute(
                "INSERT INTO api_keys (id, account_id, name, prefix, secret_hash, created_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    row.id,
                    row.account_id,
                    row.name,
                    row.prefix,
                    secret_hash,
                    row.created_ms
                ],
            )
            .map(|_| ())
        })
        .await
    }

    /// Resolve an unrevoked API key secret to (key, account); stamps `last_used_ms`.
    pub async fn key_auth(
        &self,
        secret_hash: String,
        now_ms: i64,
    ) -> Result<Option<(KeyRow, AccountRow)>> {
        self.call(move |c| {
            let found = c
                .query_row(
                    "SELECT k.id, k.account_id, k.name, k.prefix, k.created_ms, k.last_used_ms, k.revoked_ms,
                            a.id, a.email, a.created_ms, a.max_concurrent_sessions, a.session_creates_per_hour
                     FROM api_keys k JOIN accounts a ON a.id = k.account_id
                     WHERE k.secret_hash = ?1 AND k.revoked_ms IS NULL",
                    params![secret_hash.clone()],
                    |r| {
                        Ok((
                            KeyRow {
                                id: r.get(0)?,
                                account_id: r.get(1)?,
                                name: r.get(2)?,
                                prefix: r.get(3)?,
                                created_ms: r.get(4)?,
                                last_used_ms: r.get(5)?,
                                revoked_ms: r.get(6)?,
                            },
                            AccountRow {
                                id: r.get(7)?,
                                email: r.get(8)?,
                                created_ms: r.get(9)?,
                                max_concurrent_sessions: r.get(10)?,
                                session_creates_per_hour: r.get(11)?,
                            },
                        ))
                    },
                )
                .optional()?;
            if found.is_some() {
                c.execute(
                    "UPDATE api_keys SET last_used_ms = ?2 WHERE secret_hash = ?1",
                    params![secret_hash, now_ms],
                )?;
            }
            Ok(found)
        })
        .await
    }

    pub async fn list_keys(&self, account_id: String) -> Result<Vec<KeyRow>> {
        self.call(move |c| {
            let mut stmt = c.prepare(
                "SELECT id, account_id, name, prefix, created_ms, last_used_ms, revoked_ms
                 FROM api_keys WHERE account_id = ?1 ORDER BY created_ms DESC",
            )?;
            let rows = stmt.query_map(params![account_id], key_row)?;
            rows.collect()
        })
        .await
    }

    /// Revoke; Ok(false) when the key does not exist or belongs to someone else.
    pub async fn revoke_key(
        &self,
        account_id: String,
        key_id: String,
        now_ms: i64,
    ) -> Result<bool> {
        self.call(move |c| {
            let n = c.execute(
                "UPDATE api_keys SET revoked_ms = ?3 WHERE id = ?1 AND account_id = ?2 AND revoked_ms IS NULL",
                params![key_id, account_id, now_ms],
            )?;
            Ok(n > 0)
        })
        .await
    }

    // ---- billing ----

    pub async fn create_topup(&self, row: TopupRow) -> Result<()> {
        self.call(move |c| {
            c.execute(
                "INSERT INTO topups (id, account_id, amount_cents, status, provider, provider_ref, checkout_url, created_ms, paid_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![row.id, row.account_id, row.amount_cents, row.status, row.provider,
                        row.provider_ref, row.checkout_url, row.created_ms, row.paid_ms],
            )
            .map(|_| ())
        })
        .await
    }

    pub async fn topup(&self, account_id: String, id: String) -> Result<Option<TopupRow>> {
        self.call(move |c| {
            c.query_row(
                "SELECT id, account_id, amount_cents, status, provider, provider_ref, checkout_url, created_ms, paid_ms
                 FROM topups WHERE id = ?1 AND account_id = ?2",
                params![id, account_id],
                topup_row,
            )
            .optional()
        })
        .await
    }

    pub async fn list_topups(&self, account_id: String) -> Result<Vec<TopupRow>> {
        self.call(move |c| {
            let mut stmt = c.prepare(
                "SELECT id, account_id, amount_cents, status, provider, provider_ref, checkout_url, created_ms, paid_ms
                 FROM topups WHERE account_id = ?1 ORDER BY created_ms DESC",
            )?;
            let rows = stmt.query_map(params![account_id], topup_row)?;
            rows.collect()
        })
        .await
    }

    /// Mark paid and credit the ledger, atomically and idempotently: the credit row's primary
    /// key is `topup:<id>`, so a webhook and a poll racing (or a retried poll) credit once.
    pub async fn topup_paid(&self, id: String, now_ms: i64) -> Result<()> {
        self.call(move |c| {
            let tx = c.transaction()?;
            let n = tx.execute(
                "UPDATE topups SET status = 'paid', paid_ms = ?2 WHERE id = ?1 AND status = 'pending'",
                params![id, now_ms],
            )?;
            if n > 0 {
                tx.execute(
                    "INSERT OR IGNORE INTO ledger (ref, account_id, microusd, updated_ms)
                     SELECT 'topup:' || id, account_id, amount_cents * 10000, ?2 FROM topups WHERE id = ?1",
                    params![id, now_ms],
                )?;
            }
            tx.commit()
        })
        .await
    }

    /// Webhook settlement: the signed event must still match the exact Stripe Checkout
    /// Session and amount stored when this top-up was created. Returns false for an unrelated
    /// or mismatched event; a poll can continue to be the recovery path.
    pub async fn stripe_topup_paid(
        &self,
        id: String,
        provider_ref: String,
        amount_cents: i64,
        now_ms: i64,
    ) -> Result<bool> {
        self.call(move |c| {
            let tx = c.transaction()?;
            let matched: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM topups WHERE id = ?1 AND provider = 'stripe' AND provider_ref = ?2 AND amount_cents = ?3)",
                params![id, provider_ref, amount_cents],
                |r| r.get(0),
            )?;
            if matched {
                let n = tx.execute(
                    "UPDATE topups SET status = 'paid', paid_ms = ?4 WHERE id = ?1 AND provider = 'stripe' AND provider_ref = ?2 AND amount_cents = ?3 AND status = 'pending'",
                    params![id, provider_ref, amount_cents, now_ms],
                )?;
                if n > 0 {
                    tx.execute(
                        "INSERT OR IGNORE INTO ledger (ref, account_id, microusd, updated_ms)
                         SELECT 'topup:' || id, account_id, amount_cents * 10000, ?2 FROM topups WHERE id = ?1",
                        params![id, now_ms],
                    )?;
                }
            }
            tx.commit()?;
            Ok(matched)
        })
        .await
    }

    pub async fn topup_expired(&self, id: String) -> Result<()> {
        self.call(move |c| {
            c.execute(
                "UPDATE topups SET status = 'expired' WHERE id = ?1 AND status = 'pending'",
                params![id],
            )
            .map(|_| ())
        })
        .await
    }

    pub async fn stripe_topup_expired(&self, id: String, provider_ref: String) -> Result<bool> {
        self.call(move |c| {
            let n = c.execute(
                "UPDATE topups SET status = 'expired' WHERE id = ?1 AND provider = 'stripe' AND provider_ref = ?2 AND status = 'pending'",
                params![id, provider_ref],
            )?;
            Ok(n > 0)
        })
        .await
    }

    /// SUM of the ledger in micro-USD.
    pub async fn balance(&self, account_id: String) -> Result<i64> {
        self.call(move |c| {
            c.query_row(
                "SELECT COALESCE(SUM(microusd), 0) FROM ledger WHERE account_id = ?1",
                params![account_id],
                |r| r.get(0),
            )
        })
        .await
    }

    // ---- sessions and metering ----

    pub async fn insert_session(&self, row: SessionRow) -> Result<()> {
        self.call(move |c| {
            c.execute(
                "INSERT INTO sessions (id, account_id, key_id, shape, created_ms, metered_to_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    row.id,
                    row.account_id,
                    row.key_id,
                    row.shape,
                    row.created_ms,
                    row.fold.metered_to_ms
                ],
            )
            .map(|_| ())
        })
        .await
    }

    pub async fn session(&self, id: String) -> Result<Option<SessionRow>> {
        self.call(move |c| {
            c.query_row(
                &format!("{SESSION_COLS} WHERE id = ?1"),
                params![id],
                session_row,
            )
            .optional()
        })
        .await
    }

    pub async fn sessions_of(&self, account_id: String) -> Result<Vec<SessionRow>> {
        self.call(move |c| {
            let mut stmt = c.prepare(&format!(
                "{SESSION_COLS} WHERE account_id = ?1 ORDER BY created_ms DESC"
            ))?;
            let rows = stmt.query_map(params![account_id], session_row)?;
            rows.collect()
        })
        .await
    }

    pub async fn sessions_to_sweep(&self) -> Result<Vec<SessionRow>> {
        self.call(move |c| {
            let mut stmt = c.prepare(&format!("{SESSION_COLS} WHERE final = 0"))?;
            let rows = stmt.query_map([], session_row)?;
            rows.collect()
        })
        .await
    }

    /// Sessions that count against the concurrency cap: not final, and either mid-turn or
    /// holding a live or suspended hand (a released hand costs storage only).
    pub async fn live_count(&self, account_id: String) -> Result<i64> {
        self.call(move |c| {
            c.query_row(
                "SELECT COUNT(*) FROM sessions WHERE account_id = ?1 AND final = 0
                 AND (session_state = 'active' OR hand_state IN ('preparing', 'ready', 'suspended'))",
                params![account_id],
                |r| r.get(0),
            )
        })
        .await
    }

    pub async fn creates_since(&self, account_id: String, since_ms: i64) -> Result<i64> {
        self.call(move |c| {
            c.query_row(
                "SELECT COUNT(*) FROM sessions WHERE account_id = ?1 AND created_ms >= ?2",
                params![account_id, since_ms],
                |r| r.get(0),
            )
        })
        .await
    }

    /// Persist a sweep: meter state + the session's absolute usage debit, in one transaction.
    /// Fenced monotonic — a sweep that lost a race writes nothing.
    pub async fn apply_sweep(
        &self,
        session_id: String,
        account_id: String,
        fold: FoldState,
        is_final: bool,
        usage_microusd: i64,
        now_ms: i64,
    ) -> Result<bool> {
        self.call(move |c| {
            let tx = c.transaction()?;
            let n = tx.execute(
                "UPDATE sessions SET folded_seq = ?2, running_ms = ?3, turn_open_ms = ?4,
                    susp_byte_s = ?5, ws_byte_s = ?6, art_byte_s = ?7,
                    web_search_queries = ?8, metered_to_ms = ?9,
                    workspace_bytes = ?10, suspended_bytes = ?11, artifact_bytes = ?12,
                    hand_state = ?13, session_state = ?14, final = ?15
                 WHERE id = ?1 AND folded_seq <= ?2 AND metered_to_ms <= ?9 AND final = 0",
                params![
                    session_id,
                    fold.folded_seq,
                    fold.running_ms,
                    fold.turn_open_ms,
                    fold.suspended_byte_seconds,
                    fold.workspace_byte_seconds,
                    fold.artifact_byte_seconds,
                    fold.web_search_queries,
                    fold.metered_to_ms,
                    fold.workspace_bytes,
                    fold.suspended_bytes,
                    fold.artifact_bytes,
                    fold.hand_state,
                    fold.session_state,
                    is_final as i64,
                ],
            )?;
            if n > 0 {
                tx.execute(
                    "INSERT INTO ledger (ref, account_id, microusd, updated_ms)
                     VALUES ('usage:' || ?1, ?2, ?3, ?4)
                     ON CONFLICT(ref) DO UPDATE SET microusd = ?3, updated_ms = ?4",
                    params![session_id, account_id, -usage_microusd, now_ms],
                )?;
            }
            tx.commit()?;
            Ok(n > 0)
        })
        .await
    }
}

const SESSION_COLS: &str = "SELECT id, account_id, key_id, shape, created_ms, final,
    folded_seq, running_ms, turn_open_ms, susp_byte_s, ws_byte_s, art_byte_s,
    web_search_queries, metered_to_ms,
    workspace_bytes, suspended_bytes, artifact_bytes, hand_state, session_state FROM sessions";

fn account_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<AccountRow> {
    Ok(AccountRow {
        id: r.get(0)?,
        email: r.get(1)?,
        created_ms: r.get(2)?,
        max_concurrent_sessions: r.get(3)?,
        session_creates_per_hour: r.get(4)?,
    })
}

fn key_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<KeyRow> {
    Ok(KeyRow {
        id: r.get(0)?,
        account_id: r.get(1)?,
        name: r.get(2)?,
        prefix: r.get(3)?,
        created_ms: r.get(4)?,
        last_used_ms: r.get(5)?,
        revoked_ms: r.get(6)?,
    })
}

fn topup_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<TopupRow> {
    Ok(TopupRow {
        id: r.get(0)?,
        account_id: r.get(1)?,
        amount_cents: r.get(2)?,
        status: r.get(3)?,
        provider: r.get(4)?,
        provider_ref: r.get(5)?,
        checkout_url: r.get(6)?,
        created_ms: r.get(7)?,
        paid_ms: r.get(8)?,
    })
}

fn session_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRow> {
    Ok(SessionRow {
        id: r.get(0)?,
        account_id: r.get(1)?,
        key_id: r.get(2)?,
        shape: r.get(3)?,
        created_ms: r.get(4)?,
        is_final: r.get::<_, i64>(5)? != 0,
        fold: FoldState {
            folded_seq: r.get(6)?,
            running_ms: r.get(7)?,
            turn_open_ms: r.get(8)?,
            suspended_byte_seconds: r.get(9)?,
            workspace_byte_seconds: r.get(10)?,
            artifact_byte_seconds: r.get(11)?,
            web_search_queries: r.get(12)?,
            metered_to_ms: r.get(13)?,
            workspace_bytes: r.get(14)?,
            suspended_bytes: r.get(15)?,
            artifact_bytes: r.get(16)?,
            hand_state: r.get(17)?,
            session_state: r.get(18)?,
        },
    })
}

fn internal(e: rusqlite::Error) -> Error {
    Error::Internal(format!("sqlite: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acct(id: &str) -> AccountRow {
        AccountRow {
            id: id.into(),
            email: format!("{id}@example.com"),
            created_ms: 1,
            max_concurrent_sessions: 10,
            session_creates_per_hour: 30,
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn existing_ledger_is_migrated_for_web_search_metering() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        conn.execute("ALTER TABLE sessions DROP COLUMN web_search_queries", [])
            .unwrap();
        let db = Db::init(conn).unwrap();
        let columns: Vec<String> = db
            .call(|connection| {
                let mut statement = connection.prepare("PRAGMA table_info(sessions)")?;
                statement
                    .query_map([], |row| row.get(1))?
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .await
            .unwrap();
        assert!(columns.iter().any(|name| name == "web_search_queries"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn topup_credit_is_idempotent() {
        let db = Db::open_memory().unwrap();
        db.create_account(acct("acc_a"), "a@example.com".into(), "h1".into())
            .await
            .unwrap();
        db.create_topup(TopupRow {
            id: "top_1".into(),
            account_id: "acc_a".into(),
            amount_cents: 1000,
            status: "pending".into(),
            provider: "fake".into(),
            provider_ref: "r1".into(),
            checkout_url: None,
            created_ms: 2,
            paid_ms: None,
        })
        .await
        .unwrap();
        db.topup_paid("top_1".into(), 3).await.unwrap();
        db.topup_paid("top_1".into(), 4).await.unwrap(); // webhook + poll race: once
        assert_eq!(db.balance("acc_a".into()).await.unwrap(), 10_000_000);
        let t = db
            .topup("acc_a".into(), "top_1".into())
            .await
            .unwrap()
            .unwrap();
        assert_eq!((t.status.as_str(), t.paid_ms), ("paid", Some(3)));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn stripe_webhook_requires_exact_session_and_amount() {
        let db = Db::open_memory().unwrap();
        db.create_account(acct("acc_a"), "a@example.com".into(), "h1".into())
            .await
            .unwrap();
        db.create_topup(TopupRow {
            id: "top_1".into(),
            account_id: "acc_a".into(),
            amount_cents: 1000,
            status: "pending".into(),
            provider: "stripe".into(),
            provider_ref: "cs_test_1".into(),
            checkout_url: None,
            created_ms: 2,
            paid_ms: None,
        })
        .await
        .unwrap();

        assert!(
            !db.stripe_topup_paid("top_1".into(), "cs_wrong".into(), 1000, 3)
                .await
                .unwrap()
        );
        assert!(
            !db.stripe_topup_paid("top_1".into(), "cs_test_1".into(), 999, 3)
                .await
                .unwrap()
        );
        assert_eq!(db.balance("acc_a".into()).await.unwrap(), 0);
        assert!(
            db.stripe_topup_paid("top_1".into(), "cs_test_1".into(), 1000, 4)
                .await
                .unwrap()
        );
        assert!(
            db.stripe_topup_paid("top_1".into(), "cs_test_1".into(), 1000, 5)
                .await
                .unwrap()
        );
        assert_eq!(db.balance("acc_a".into()).await.unwrap(), 10_000_000);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn usage_debit_overwrites_never_accumulates() {
        let db = Db::open_memory().unwrap();
        db.create_account(acct("acc_a"), "a@example.com".into(), "h1".into())
            .await
            .unwrap();
        let mut row = SessionRow {
            id: "ses_1".into(),
            account_id: "acc_a".into(),
            key_id: "key_1".into(),
            shape: "1gb".into(),
            created_ms: 5,
            is_final: false,
            fold: FoldState::default(),
        };
        db.insert_session(row.clone()).await.unwrap();
        row.fold.folded_seq = 10;
        row.fold.metered_to_ms = 100;
        assert!(
            db.apply_sweep(
                "ses_1".into(),
                "acc_a".into(),
                row.fold.clone(),
                false,
                500,
                100
            )
            .await
            .unwrap()
        );
        row.fold.folded_seq = 20;
        row.fold.metered_to_ms = 200;
        assert!(
            db.apply_sweep(
                "ses_1".into(),
                "acc_a".into(),
                row.fold.clone(),
                false,
                800,
                200
            )
            .await
            .unwrap()
        );
        assert_eq!(db.balance("acc_a".into()).await.unwrap(), -800);
        // A stale sweep (lower fence) writes nothing.
        row.fold.folded_seq = 15;
        row.fold.metered_to_ms = 150;
        assert!(
            !db.apply_sweep(
                "ses_1".into(),
                "acc_a".into(),
                row.fold.clone(),
                false,
                9999,
                150
            )
            .await
            .unwrap()
        );
        assert_eq!(db.balance("acc_a".into()).await.unwrap(), -800);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn key_auth_resolves_and_revocation_bites() {
        let db = Db::open_memory().unwrap();
        db.create_account(acct("acc_a"), "a@example.com".into(), "h1".into())
            .await
            .unwrap();
        db.create_key(
            KeyRow {
                id: "key_1".into(),
                account_id: "acc_a".into(),
                name: "laptop".into(),
                prefix: "aex_sk_abc".into(),
                created_ms: 2,
                last_used_ms: None,
                revoked_ms: None,
            },
            "sh1".into(),
        )
        .await
        .unwrap();
        let (k, a) = db.key_auth("sh1".into(), 9).await.unwrap().unwrap();
        assert_eq!((k.id.as_str(), a.id.as_str()), ("key_1", "acc_a"));
        assert!(
            db.revoke_key("acc_a".into(), "key_1".into(), 10)
                .await
                .unwrap()
        );
        assert!(db.key_auth("sh1".into(), 11).await.unwrap().is_none());
        assert!(
            !db.revoke_key("acc_a".into(), "key_1".into(), 12)
                .await
                .unwrap()
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn duplicate_email_is_a_conflict() {
        let db = Db::open_memory().unwrap();
        db.create_account(acct("acc_a"), "dup@example.com".into(), "h1".into())
            .await
            .unwrap();
        let err = db
            .create_account(acct("acc_b"), "dup@example.com".into(), "h2".into())
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Conflict(_)), "{err}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn live_count_ignores_released_and_final() {
        let db = Db::open_memory().unwrap();
        db.create_account(acct("acc_a"), "a@example.com".into(), "h1".into())
            .await
            .unwrap();
        for (id, hand, state, is_final) in [
            ("ses_a", "ready", "idle", false),
            ("ses_b", "suspended", "idle", false),
            ("ses_c", "released", "idle", false),
            ("ses_d", "ready", "idle", true),
        ] {
            let mut row = SessionRow {
                id: id.into(),
                account_id: "acc_a".into(),
                key_id: "key_1".into(),
                shape: "1gb".into(),
                created_ms: 5,
                is_final: false,
                fold: FoldState::default(),
            };
            db.insert_session(row.clone()).await.unwrap();
            row.fold.hand_state = hand.into();
            row.fold.session_state = state.into();
            db.apply_sweep(id.into(), "acc_a".into(), row.fold, is_final, 0, 6)
                .await
                .unwrap();
        }
        assert_eq!(db.live_count("acc_a".into()).await.unwrap(), 2);
    }
}
