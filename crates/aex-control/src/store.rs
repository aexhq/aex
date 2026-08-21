//! SQLite persistence: accounts, key custody (hashes only), the prepaid ledger, session
//! ownership and meter state.
//!
//! Money semantics: the ledger holds one signed row per `ref` in micro-USD.
//! `topup:<id>` rows are credits, inserted exactly once (idempotent by primary key).
//! `grant:<id>` rows are operator-issued service credits, also inserted exactly once.
//! `usage:<session>` rows are debits, OVERWRITTEN by each sweep with the absolute rated total
//! for that session — never incremented — so a replayed or racing sweep cannot double-bill.
//! Balance = SUM(ledger). Meter-state updates are fenced monotonic (`folded_seq`,
//! `metered_to_ms` never move backwards), so concurrent sweeps are safe: the loser's write is
//! simply skipped. `storage_transition_ms` is deliberately separate: a newly replayed durable
//! transition may predate an earlier wall-clock estimate and must still correct that estimate.

use std::path::Path;
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, OptionalExtension, params};

use crate::rating::FoldState;
use crate::{Error, Result};

const KEY_LAST_USED_WRITE_INTERVAL_MS: i64 = 60_000;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS waitlist (
  email TEXT PRIMARY KEY COLLATE NOCASE,
  status TEXT NOT NULL CHECK (status IN ('waiting', 'invited', 'joined')),
  invite_hash TEXT UNIQUE,
  created_ms INTEGER NOT NULL,
  invited_ms INTEGER,
  joined_ms INTEGER
);
CREATE INDEX IF NOT EXISTS waitlist_status ON waitlist(status, created_ms);
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
CREATE UNIQUE INDEX IF NOT EXISTS topups_provider_ref ON topups(provider, provider_ref);
CREATE TABLE IF NOT EXISTS refunds (
  id TEXT PRIMARY KEY,
  request_key TEXT NOT NULL UNIQUE,
  topup_id TEXT NOT NULL REFERENCES topups(id),
  account_id TEXT NOT NULL REFERENCES accounts(id),
  amount_cents INTEGER NOT NULL CHECK (amount_cents > 0),
  status TEXT NOT NULL CHECK (status IN ('pending', 'succeeded', 'failed')),
  provider_ref TEXT,
  failure_reason TEXT,
  created_ms INTEGER NOT NULL,
  updated_ms INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS refunds_topup ON refunds(topup_id, status);
CREATE TABLE IF NOT EXISTS credit_grants (
  id TEXT PRIMARY KEY,
  request_key TEXT NOT NULL UNIQUE,
  account_id TEXT NOT NULL REFERENCES accounts(id),
  amount_cents INTEGER NOT NULL CHECK (amount_cents > 0),
  reason TEXT NOT NULL,
  created_ms INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS credit_grants_account ON credit_grants(account_id, created_ms);
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
  parent_id TEXT,
  root_id TEXT NOT NULL,
  depth INTEGER NOT NULL CHECK (depth BETWEEN 0 AND 8),
  shape TEXT NOT NULL,
  created_ms INTEGER NOT NULL,
  final INTEGER NOT NULL DEFAULT 0,
  folded_seq INTEGER NOT NULL DEFAULT 0,
  running_ms INTEGER NOT NULL DEFAULT 0,
  turn_open_ms INTEGER,
  session_storage_byte_s INTEGER NOT NULL DEFAULT 0,
  session_storage_byte_ms_remainder INTEGER NOT NULL DEFAULT 0,
  web_search_queries INTEGER NOT NULL DEFAULT 0,
  storage_transition_ms INTEGER NOT NULL DEFAULT 0,
  metered_to_ms INTEGER NOT NULL DEFAULT 0,
  session_storage_bytes INTEGER NOT NULL DEFAULT 0,
  upload_reserved_bytes INTEGER NOT NULL DEFAULT 0,
  session_state TEXT NOT NULL DEFAULT 'open'
);
CREATE INDEX IF NOT EXISTS sessions_account ON sessions(account_id);
CREATE INDEX IF NOT EXISTS sessions_parent ON sessions(account_id, parent_id);
CREATE INDEX IF NOT EXISTS sessions_open_turn ON sessions(account_id, final, turn_open_ms);
CREATE INDEX IF NOT EXISTS sessions_meter_due ON sessions(account_id, final, metered_to_ms);
CREATE TABLE IF NOT EXISTS session_create_requests (
  account_id TEXT NOT NULL REFERENCES accounts(id),
  request_key_hash TEXT NOT NULL,
  request_hash TEXT NOT NULL,
  session_id TEXT NOT NULL REFERENCES sessions(id),
  created_ms INTEGER NOT NULL,
  PRIMARY KEY(account_id, request_key_hash)
);
CREATE TABLE IF NOT EXISTS session_create_intents (
  account_id TEXT NOT NULL REFERENCES accounts(id),
  request_key_hash TEXT NOT NULL,
  request_hash TEXT NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('dispatching', 'uncertain')),
  covered_session_id TEXT,
  created_ms INTEGER NOT NULL,
  updated_ms INTEGER NOT NULL,
  PRIMARY KEY(account_id, request_key_hash)
);
CREATE UNIQUE INDEX IF NOT EXISTS session_create_intents_covered
  ON session_create_intents(covered_session_id) WHERE covered_session_id IS NOT NULL;
CREATE TABLE IF NOT EXISTS session_discovery (
  account_id TEXT PRIMARY KEY REFERENCES accounts(id),
  watermark_ms INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS session_deletions (
  session_id TEXT PRIMARY KEY,
  account_id TEXT NOT NULL REFERENCES accounts(id),
  anchor_id TEXT NOT NULL,
  phase TEXT NOT NULL CHECK (phase IN ('ending', 'settling', 'purging', 'awaiting_purge', 'succeeded')),
  accepted_ms INTEGER NOT NULL,
  updated_ms INTEGER NOT NULL,
  completed_ms INTEGER,
  last_error TEXT,
  claimed_ms INTEGER,
  attempts INTEGER NOT NULL DEFAULT 0,
  next_attempt_ms INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS session_deletions_account ON session_deletions(account_id, phase, accepted_ms);
CREATE TABLE IF NOT EXISTS session_deletion_dependencies (
  session_id TEXT NOT NULL REFERENCES session_deletions(session_id),
  dependency_id TEXT NOT NULL REFERENCES session_deletions(session_id),
  PRIMARY KEY(session_id, dependency_id),
  CHECK(session_id != dependency_id)
);
CREATE TABLE IF NOT EXISTS output_requests (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL REFERENCES sessions(id),
  schema_hash TEXT NOT NULL,
  schema_json TEXT NOT NULL,
  max_attempts INTEGER NOT NULL CHECK (max_attempts BETWEEN 1 AND 3),
  attempts INTEGER NOT NULL DEFAULT 0,
  status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'completed', 'failed')),
  turn_id TEXT,
  accepted_json TEXT,
  result_json TEXT,
  error_json TEXT,
  idempotency_key_hash TEXT,
  request_hash TEXT NOT NULL,
  created_ms INTEGER NOT NULL,
  UNIQUE(session_id, idempotency_key_hash)
);
CREATE INDEX IF NOT EXISTS output_requests_session ON output_requests(session_id, created_ms);
CREATE TABLE IF NOT EXISTS external_tool_calls (
  session_id TEXT NOT NULL,
  call_id TEXT NOT NULL,
  output_id TEXT NOT NULL REFERENCES output_requests(id),
  response_json TEXT NOT NULL,
  created_ms INTEGER NOT NULL,
  PRIMARY KEY(session_id, call_id)
);
CREATE TABLE IF NOT EXISTS hosted_tool_calls (
  session_id TEXT NOT NULL,
  call_id TEXT NOT NULL,
  request_hash TEXT NOT NULL,
  response_json TEXT NOT NULL,
  created_ms INTEGER NOT NULL,
  PRIMARY KEY(session_id, call_id)
);
";

#[derive(Debug, Clone)]
pub struct WaitlistRow {
    pub email: String,
    pub status: String,
    pub created_ms: i64,
    pub invited_ms: Option<i64>,
    pub joined_ms: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct AccountRow {
    pub id: String,
    pub email: String,
    pub created_ms: i64,
    pub max_concurrent_sessions: i64,
    pub session_creates_per_hour: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletionRow {
    pub session_id: String,
    pub account_id: String,
    /// The executable deletion job. An alias has a different anchor and never calls Brain itself.
    pub anchor_id: String,
    pub phase: String,
    pub accepted_ms: i64,
    pub updated_ms: i64,
    pub completed_ms: Option<i64>,
    pub last_error: Option<String>,
    pub claimed_ms: Option<i64>,
    pub attempts: i64,
    pub next_attempt_ms: i64,
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
pub struct RefundRow {
    pub id: String,
    pub request_key: String,
    pub topup_id: String,
    pub account_id: String,
    pub amount_cents: i64,
    pub status: String,
    pub provider_ref: Option<String>,
    pub failure_reason: Option<String>,
    pub created_ms: i64,
    pub updated_ms: i64,
}

#[derive(Debug, Clone)]
pub struct CreditGrantRow {
    pub id: String,
    pub request_key: String,
    pub account_id: String,
    pub email: String,
    pub amount_cents: i64,
    pub reason: String,
    pub created_ms: i64,
}

enum GrantCreditOutcome {
    Created(CreditGrantRow),
    Existing(CreditGrantRow),
    RequestMismatch,
    AccountNotFound,
}

enum BeginRefundOutcome {
    Ready(RefundRow),
    RequestMismatch,
    TopupNotFound,
    TopupNotPaid,
    ProviderMismatch,
    ExceedsTopup,
    InsufficientBalance(i64),
}

enum RefundTransitionOutcome {
    Ready(RefundRow),
    NotFound,
    Conflict,
}

#[derive(Debug, Clone)]
pub struct SessionRow {
    pub id: String,
    pub account_id: String,
    pub key_id: String,
    pub parent_id: Option<String>,
    pub root_id: String,
    pub depth: i64,
    pub shape: String,
    pub created_ms: i64,
    pub is_final: bool,
    pub fold: FoldState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCreateRow {
    pub request_hash: String,
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCreateIntentRow {
    pub request_hash: String,
    pub covered: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionCreateIntent {
    Created,
    Existing,
    Conflict,
    RetainedRootLimit,
}

#[derive(Debug, Clone)]
pub struct OutputRequestRow {
    pub id: String,
    pub session_id: String,
    pub schema_hash: String,
    pub schema_json: String,
    pub max_attempts: i64,
    pub attempts: i64,
    pub status: String,
    pub turn_id: Option<String>,
    pub accepted_json: Option<String>,
    pub result_json: Option<String>,
    pub error_json: Option<String>,
    pub idempotency_key_hash: Option<String>,
    pub request_hash: String,
    pub created_ms: i64,
}

#[derive(Debug, Clone)]
pub enum BeginOutput {
    Created(OutputRequestRow),
    Existing(OutputRequestRow),
    RequestMismatch,
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
        // The production ledger lives on EFS (NFS). SQLite WAL relies on a shared-memory
        // wal-index and is not safe on a network filesystem, so keep the database in the
        // rollback-journal mode. A persisted WAL database is converted when the sole control
        // task starts; refusing to remain in WAL mode keeps a bad deployment from serving.
        conn.pragma_update(None, "journal_mode", "DELETE")
            .map_err(internal)?;
        let journal_mode: String = conn
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .map_err(internal)?;
        if journal_mode.eq_ignore_ascii_case("wal") {
            return Err(Error::Internal(
                "sqlite: WAL mode is unsafe for the network-backed ledger".into(),
            ));
        }
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(internal)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(internal)?;
        conn.execute_batch(SCHEMA).map_err(internal)?;
        let integrity: String = conn
            .query_row("PRAGMA quick_check(1)", [], |row| row.get(0))
            .map_err(internal)?;
        if integrity != "ok" {
            return Err(Error::Internal(format!(
                "sqlite: ledger integrity check failed: {integrity}"
            )));
        }
        // EFS carries the SQLite ledger across task and image upgrades. Additive schema
        // changes therefore migrate in place; new databases already contain the column.
        let session_columns = {
            let mut stmt = conn
                .prepare("PRAGMA table_info(sessions)")
                .map_err(internal)?;
            let names = stmt
                .query_map([], |r| r.get::<_, String>(1))
                .map_err(internal)?;
            names
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(internal)?
        };
        if !session_columns
            .iter()
            .any(|name| name == "web_search_queries")
        {
            conn.execute(
                "ALTER TABLE sessions ADD COLUMN web_search_queries INTEGER NOT NULL DEFAULT 0",
                [],
            )
            .map_err(internal)?;
        }
        if !session_columns.iter().any(|name| name == "parent_id") {
            conn.execute("ALTER TABLE sessions ADD COLUMN parent_id TEXT", [])
                .map_err(internal)?;
        }
        if !session_columns.iter().any(|name| name == "root_id") {
            conn.execute(
                "ALTER TABLE sessions ADD COLUMN root_id TEXT NOT NULL DEFAULT ''",
                [],
            )
            .map_err(internal)?;
            conn.execute("UPDATE sessions SET root_id = id WHERE root_id = ''", [])
                .map_err(internal)?;
        }
        if !session_columns.iter().any(|name| name == "depth") {
            conn.execute(
                "ALTER TABLE sessions ADD COLUMN depth INTEGER NOT NULL DEFAULT 0",
                [],
            )
            .map_err(internal)?;
        }
        if !session_columns
            .iter()
            .any(|name| name == "upload_reserved_bytes")
        {
            conn.execute(
                "ALTER TABLE sessions ADD COLUMN upload_reserved_bytes INTEGER NOT NULL DEFAULT 0",
                [],
            )
            .map_err(internal)?;
        }
        if !session_columns
            .iter()
            .any(|name| name == "session_storage_byte_ms_remainder")
        {
            conn.execute(
                "ALTER TABLE sessions ADD COLUMN session_storage_byte_ms_remainder \
                 INTEGER NOT NULL DEFAULT 0",
                [],
            )
            .map_err(internal)?;
        }
        if !session_columns
            .iter()
            .any(|name| name == "storage_transition_ms")
        {
            conn.execute(
                "ALTER TABLE sessions ADD COLUMN storage_transition_ms INTEGER NOT NULL DEFAULT 0",
                [],
            )
            .map_err(internal)?;
        }
        let deletion_columns = {
            let mut statement = conn
                .prepare("PRAGMA table_info(session_deletions)")
                .map_err(internal)?;
            statement
                .query_map([], |record| record.get::<_, String>(1))
                .map_err(internal)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(internal)?
        };
        if !deletion_columns.iter().any(|name| name == "anchor_id") {
            conn.execute(
                "ALTER TABLE session_deletions ADD COLUMN anchor_id TEXT NOT NULL DEFAULT ''",
                [],
            )
            .map_err(internal)?;
            conn.execute(
                "UPDATE session_deletions SET anchor_id = session_id WHERE anchor_id = ''",
                [],
            )
            .map_err(internal)?;
        }
        if !deletion_columns.iter().any(|name| name == "claimed_ms") {
            conn.execute(
                "ALTER TABLE session_deletions ADD COLUMN claimed_ms INTEGER",
                [],
            )
            .map_err(internal)?;
        }
        if !deletion_columns.iter().any(|name| name == "attempts") {
            conn.execute(
                "ALTER TABLE session_deletions ADD COLUMN attempts INTEGER NOT NULL DEFAULT 0",
                [],
            )
            .map_err(internal)?;
        }
        if !deletion_columns
            .iter()
            .any(|name| name == "next_attempt_ms")
        {
            conn.execute(
                "ALTER TABLE session_deletions ADD COLUMN next_attempt_ms INTEGER NOT NULL DEFAULT 0",
                [],
            )
            .map_err(internal)?;
        }
        conn.execute(
            "CREATE INDEX IF NOT EXISTS session_deletions_anchor
             ON session_deletions(anchor_id, phase)",
            [],
        )
        .map_err(internal)?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS session_deletions_due
             ON session_deletions(phase, next_attempt_ms, accepted_ms)",
            [],
        )
        .map_err(internal)?;
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

    /// Add an email once. Repeated submissions preserve the operator-owned lifecycle state.
    pub async fn join_waitlist(&self, email: String, now_ms: i64) -> Result<WaitlistRow> {
        self.call(move |c| {
            c.execute(
                "INSERT INTO waitlist (email, status, created_ms)
                 VALUES (?1, 'waiting', ?2)
                 ON CONFLICT(email) DO NOTHING",
                params![email, now_ms],
            )?;
            c.query_row(
                "SELECT email, status, created_ms, invited_ms, joined_ms
                 FROM waitlist WHERE email = ?1",
                params![email],
                waitlist_row,
            )
        })
        .await
    }

    pub async fn list_waitlist(&self) -> Result<Vec<WaitlistRow>> {
        self.call(|c| {
            let mut stmt = c.prepare(
                "SELECT email, status, created_ms, invited_ms, joined_ms
                 FROM waitlist ORDER BY created_ms DESC",
            )?;
            stmt.query_map([], waitlist_row)?.collect()
        })
        .await
    }

    /// Create or rotate an invitation. Joined rows cannot be invited again.
    pub async fn invite_waitlist(
        &self,
        email: String,
        invite_hash: String,
        now_ms: i64,
    ) -> Result<Option<WaitlistRow>> {
        self.call(move |c| {
            let changed = c.execute(
                "UPDATE waitlist
                 SET status = 'invited', invite_hash = ?2, invited_ms = ?3
                 WHERE email = ?1 AND status != 'joined'",
                params![email, invite_hash, now_ms],
            )?;
            if changed == 0 {
                return Ok(None);
            }
            c.query_row(
                "SELECT email, status, created_ms, invited_ms, joined_ms
                 FROM waitlist WHERE email = ?1",
                params![email],
                waitlist_row,
            )
            .optional()
        })
        .await
    }

    /// Consume an invitation and create its account in one SQLite transaction.
    pub async fn create_invited_account(
        &self,
        row: AccountRow,
        email: String,
        token_hash: String,
        invite_hash: String,
        joined_ms: i64,
    ) -> Result<()> {
        let result = self
            .call(move |c| {
                let tx = c.transaction()?;
                let invited = tx
                    .query_row(
                        "SELECT 1 FROM waitlist
                         WHERE email = ?1 AND status = 'invited' AND invite_hash = ?2",
                        params![email, invite_hash],
                        |_| Ok(()),
                    )
                    .optional()?
                    .is_some();
                if !invited {
                    return Ok(false);
                }
                tx.execute(
                    "INSERT INTO accounts (id, email, token_hash, created_ms, max_concurrent_sessions, session_creates_per_hour)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![row.id, email, token_hash, row.created_ms, row.max_concurrent_sessions, row.session_creates_per_hour],
                )?;
                tx.execute(
                    "UPDATE waitlist
                     SET status = 'joined', invite_hash = NULL, joined_ms = ?2
                     WHERE email = ?1",
                    params![email, joined_ms],
                )?;
                tx.commit()?;
                Ok(true)
            })
            .await;
        match result {
            Ok(true) => Ok(()),
            Ok(false) => Err(Error::Forbidden(
                "a valid one-time alpha invitation is required".into(),
            )),
            Err(Error::Internal(message)) if message.contains("UNIQUE") => Err(Error::Conflict(
                "an account with this email already exists".into(),
            )),
            Err(error) => Err(error),
        }
    }

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

    /// Resolve an unrevoked API key secret to (key, account). The audit timestamp is deliberately
    /// write-coalesced: hosted SQLite uses a rollback journal on EFS, so rewriting it for every
    /// proxied session request would turn authentication into a forced-write hot path.
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
            if found.as_ref().is_some_and(|(key, _)| {
                key.last_used_ms.is_none_or(|last_used_ms| {
                    now_ms.saturating_sub(last_used_ms) >= KEY_LAST_USED_WRITE_INTERVAL_MS
                })
            }) {
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

    /// Append operator-issued service credit once. The request key owns idempotency and every
    /// successful grant has one durable audit row plus one immutable ledger row in the same
    /// transaction. Grants are deliberately separate from paid top-ups and Stripe refunds.
    pub async fn grant_credit(&self, proposed: CreditGrantRow) -> Result<(CreditGrantRow, bool)> {
        let microusd = proposed
            .amount_cents
            .checked_mul(10_000)
            .ok_or_else(|| Error::Invalid("credit grant amount is too large".into()))?;
        let result = self
            .call(move |c| {
                let tx = c.transaction()?;
                if let Some(existing) = tx
                    .query_row(
                        &format!("{CREDIT_GRANT_COLS} WHERE g.request_key = ?1"),
                        params![&proposed.request_key],
                        credit_grant_row,
                    )
                    .optional()?
                {
                    if existing.email.eq_ignore_ascii_case(&proposed.email)
                        && existing.amount_cents == proposed.amount_cents
                        && existing.reason == proposed.reason
                    {
                        return Ok(GrantCreditOutcome::Existing(existing));
                    }
                    return Ok(GrantCreditOutcome::RequestMismatch);
                }

                let account = tx
                    .query_row(
                        "SELECT id, email FROM accounts WHERE email = ?1 COLLATE NOCASE",
                        params![&proposed.email],
                        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                    )
                    .optional()?;
                let Some((account_id, email)) = account else {
                    return Ok(GrantCreditOutcome::AccountNotFound);
                };
                let grant = CreditGrantRow {
                    account_id,
                    email,
                    ..proposed
                };
                tx.execute(
                    "INSERT INTO credit_grants
                     (id, request_key, account_id, amount_cents, reason, created_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        &grant.id,
                        &grant.request_key,
                        &grant.account_id,
                        grant.amount_cents,
                        &grant.reason,
                        grant.created_ms
                    ],
                )?;
                tx.execute(
                    "INSERT INTO ledger (ref, account_id, microusd, updated_ms)
                     VALUES ('grant:' || ?1, ?2, ?3, ?4)",
                    params![&grant.id, &grant.account_id, microusd, grant.created_ms],
                )?;
                tx.commit()?;
                Ok(GrantCreditOutcome::Created(grant))
            })
            .await?;
        match result {
            GrantCreditOutcome::Created(row) => Ok((row, true)),
            GrantCreditOutcome::Existing(row) => Ok((row, false)),
            GrantCreditOutcome::RequestMismatch => Err(Error::Conflict(
                "Idempotency-Key was already used with different credit-grant fields".into(),
            )),
            GrantCreditOutcome::AccountNotFound => Err(Error::NotFound),
        }
    }

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

    /// Checkout-return lookup. The provider reference is an unguessable Stripe Checkout Session
    /// id; callers receive only an HTTP status and never this row's account or payment details.
    pub async fn stripe_topup_by_provider_ref(
        &self,
        provider_ref: String,
    ) -> Result<Option<TopupRow>> {
        self.call(move |c| {
            c.query_row(
                "SELECT id, account_id, amount_cents, status, provider, provider_ref, checkout_url, created_ms, paid_ms
                 FROM topups WHERE provider = 'stripe' AND provider_ref = ?1",
                params![provider_ref],
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

    /// Operator lookup used to settle support-requested refunds. Customer reads remain scoped by
    /// account in `topup` and `list_topups`.
    pub async fn operator_topup(&self, id: String) -> Result<Option<TopupRow>> {
        self.call(move |c| {
            c.query_row(
                "SELECT id, account_id, amount_cents, status, provider, provider_ref, checkout_url, created_ms, paid_ms
                 FROM topups WHERE id = ?1",
                params![id],
                topup_row,
            )
            .optional()
        })
        .await
    }

    pub async fn refund_by_request_key(&self, request_key: String) -> Result<Option<RefundRow>> {
        self.call(move |c| {
            c.query_row(
                &format!("{REFUND_COLS} WHERE request_key = ?1"),
                params![request_key],
                refund_row,
            )
            .optional()
        })
        .await
    }

    /// Reserve unused credit before calling the payment provider. The request key is unique, so
    /// an operator retry either returns the original row or conflicts if its fields changed.
    pub async fn begin_refund(
        &self,
        row: RefundRow,
        expected_provider: String,
    ) -> Result<RefundRow> {
        let result = self
            .call(move |c| {
                let tx = c.transaction()?;
                if let Some(existing) = tx
                    .query_row(
                        &format!("{REFUND_COLS} WHERE request_key = ?1"),
                        params![row.request_key],
                        refund_row,
                    )
                    .optional()?
                {
                    if existing.topup_id == row.topup_id
                        && existing.amount_cents == row.amount_cents
                    {
                        return Ok(BeginRefundOutcome::Ready(existing));
                    }
                    return Ok(BeginRefundOutcome::RequestMismatch);
                }

                let topup = tx
                    .query_row(
                        "SELECT account_id, amount_cents, status, provider FROM topups WHERE id = ?1",
                        params![row.topup_id],
                        |r| {
                            Ok((
                                r.get::<_, String>(0)?,
                                r.get::<_, i64>(1)?,
                                r.get::<_, String>(2)?,
                                r.get::<_, String>(3)?,
                            ))
                        },
                    )
                    .optional()?;
                let Some((account_id, topup_cents, topup_status, provider)) = topup else {
                    return Ok(BeginRefundOutcome::TopupNotFound);
                };
                if topup_status != "paid" {
                    return Ok(BeginRefundOutcome::TopupNotPaid);
                }
                if provider != expected_provider {
                    return Ok(BeginRefundOutcome::ProviderMismatch);
                }
                let committed_cents: i64 = tx.query_row(
                    "SELECT COALESCE(SUM(amount_cents), 0) FROM refunds
                     WHERE topup_id = ?1 AND status IN ('pending', 'succeeded')",
                    params![row.topup_id],
                    |r| r.get(0),
                )?;
                if row.amount_cents > topup_cents - committed_cents {
                    return Ok(BeginRefundOutcome::ExceedsTopup);
                }
                let balance: i64 = tx.query_row(
                    "SELECT COALESCE(SUM(microusd), 0) FROM ledger WHERE account_id = ?1",
                    params![account_id],
                    |r| r.get(0),
                )?;
                let reserve_microusd = row.amount_cents * 10_000;
                if balance < reserve_microusd {
                    return Ok(BeginRefundOutcome::InsufficientBalance(balance));
                }

                let mut row = row;
                row.account_id = account_id;
                tx.execute(
                    "INSERT INTO refunds
                     (id, request_key, topup_id, account_id, amount_cents, status, provider_ref,
                      failure_reason, created_ms, updated_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5, 'pending', NULL, NULL, ?6, ?6)",
                    params![
                        row.id,
                        row.request_key,
                        row.topup_id,
                        row.account_id,
                        row.amount_cents,
                        row.created_ms
                    ],
                )?;
                tx.execute(
                    "INSERT INTO ledger (ref, account_id, microusd, updated_ms)
                     VALUES ('refund:' || ?1, ?2, ?3, ?4)",
                    params![
                        row.id,
                        row.account_id,
                        -reserve_microusd,
                        row.created_ms
                    ],
                )?;
                tx.commit()?;
                Ok(BeginRefundOutcome::Ready(row))
            })
            .await?;

        match result {
            BeginRefundOutcome::Ready(row) => Ok(row),
            BeginRefundOutcome::RequestMismatch => Err(Error::Conflict(
                "Idempotency-Key was already used with different refund fields".into(),
            )),
            BeginRefundOutcome::TopupNotFound => Err(Error::NotFound),
            BeginRefundOutcome::TopupNotPaid => {
                Err(Error::Conflict("only a paid top-up can be refunded".into()))
            }
            BeginRefundOutcome::ProviderMismatch => Err(Error::Conflict(
                "top-up payment provider is not active".into(),
            )),
            BeginRefundOutcome::ExceedsTopup => Err(Error::Conflict(
                "amount exceeds the unrefunded portion of this top-up".into(),
            )),
            BeginRefundOutcome::InsufficientBalance(balance) => {
                Err(Error::InsufficientBalance(format!(
                    "only {} of unused account credit is available",
                    crate::usd_display(balance)
                )))
            }
        }
    }

    pub async fn refund_pending(
        &self,
        id: String,
        provider_ref: String,
        now_ms: i64,
    ) -> Result<RefundRow> {
        self.transition_refund(id, Some(provider_ref), "pending", None, now_ms)
            .await
    }

    pub async fn refund_succeeded(
        &self,
        id: String,
        provider_ref: String,
        now_ms: i64,
    ) -> Result<RefundRow> {
        self.transition_refund(id, Some(provider_ref), "succeeded", None, now_ms)
            .await
    }

    pub async fn refund_failed(
        &self,
        id: String,
        provider_ref: Option<String>,
        reason: String,
        now_ms: i64,
    ) -> Result<RefundRow> {
        self.transition_refund(id, provider_ref, "failed", Some(reason), now_ms)
            .await
    }

    async fn transition_refund(
        &self,
        id: String,
        provider_ref: Option<String>,
        target: &'static str,
        failure_reason: Option<String>,
        now_ms: i64,
    ) -> Result<RefundRow> {
        let result = self
            .call(move |c| {
                let tx = c.transaction()?;
                let existing = tx
                    .query_row(
                        &format!("{REFUND_COLS} WHERE id = ?1"),
                        params![id],
                        refund_row,
                    )
                    .optional()?;
                let Some(existing) = existing else {
                    return Ok(RefundTransitionOutcome::NotFound);
                };
                if existing
                    .provider_ref
                    .as_ref()
                    .zip(provider_ref.as_ref())
                    .is_some_and(|(stored, supplied)| stored != supplied)
                {
                    return Ok(RefundTransitionOutcome::Conflict);
                }
                if existing.status != "pending" && existing.status != target {
                    return Ok(RefundTransitionOutcome::Conflict);
                }
                if existing.status == "pending" {
                    tx.execute(
                        "UPDATE refunds
                         SET status = ?2, provider_ref = COALESCE(provider_ref, ?3),
                             failure_reason = ?4, updated_ms = ?5
                         WHERE id = ?1 AND status = 'pending'",
                        params![id, target, provider_ref, failure_reason, now_ms],
                    )?;
                    if target == "failed" {
                        tx.execute(
                            "DELETE FROM ledger WHERE ref = 'refund:' || ?1",
                            params![id],
                        )?;
                    }
                }
                let updated = tx.query_row(
                    &format!("{REFUND_COLS} WHERE id = ?1"),
                    params![id],
                    refund_row,
                )?;
                tx.commit()?;
                Ok(RefundTransitionOutcome::Ready(updated))
            })
            .await?;
        match result {
            RefundTransitionOutcome::Ready(row) => Ok(row),
            RefundTransitionOutcome::NotFound => Err(Error::NotFound),
            RefundTransitionOutcome::Conflict => Err(Error::Conflict(
                "refund provider state conflicts with the recorded attempt".into(),
            )),
        }
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
            let tx = c.transaction()?;
            insert_session_record(&tx, &row)?;
            tx.commit()
        })
        .await
    }

    pub async fn session_create_request(
        &self,
        account_id: String,
        request_key_hash: String,
    ) -> Result<Option<SessionCreateRow>> {
        self.call(move |connection| {
            connection
                .query_row(
                    "SELECT request_hash, session_id FROM session_create_requests
                     WHERE account_id = ?1 AND request_key_hash = ?2",
                    params![account_id, request_key_hash],
                    |record| {
                        Ok(SessionCreateRow {
                            request_hash: record.get(0)?,
                            session_id: record.get(1)?,
                        })
                    },
                )
                .optional()
        })
        .await
    }

    pub async fn session_create_intent(
        &self,
        account_id: String,
        request_key_hash: String,
    ) -> Result<Option<SessionCreateIntentRow>> {
        self.call(move |connection| {
            connection
                .query_row(
                    "SELECT request_hash, covered_session_id IS NOT NULL
                     FROM session_create_intents
                     WHERE account_id = ?1 AND request_key_hash = ?2",
                    params![account_id, request_key_hash],
                    |record| {
                        Ok(SessionCreateIntentRow {
                            request_hash: record.get(0)?,
                            covered: record.get(1)?,
                        })
                    },
                )
                .optional()
        })
        .await
    }

    /// Durably reserve one hosted create identity before it can reach Brain. A process crash or
    /// ambiguous response leaves this row as a conservative root slot; only the identical body
    /// may resume it. There is deliberately no time-only expiry because the tenant GSI is
    /// eventually consistent and cannot prove that an old request did not commit.
    pub async fn ensure_session_create_intent(
        &self,
        account_id: String,
        request_key_hash: String,
        request_hash: String,
        now_ms: i64,
        max_retained_roots: i64,
    ) -> Result<SessionCreateIntent> {
        self.call(move |connection| {
            let transaction = connection.transaction()?;
            let existing = transaction
                .query_row(
                    "SELECT request_hash FROM session_create_intents
                     WHERE account_id = ?1 AND request_key_hash = ?2",
                    params![&account_id, &request_key_hash],
                    |record| record.get::<_, String>(0),
                )
                .optional()?;
            let outcome = match existing {
                Some(existing_hash) if existing_hash == request_hash => {
                    SessionCreateIntent::Existing
                }
                Some(_) => SessionCreateIntent::Conflict,
                None => {
                    // This count and the new intent are one SQLite transaction. A retained root
                    // consumes its slot through open/ending/ended/failed/deleting and is released
                    // only when physical deletion marks the local tombstone final. An uncovered
                    // ambiguous intent is one additional possible root; a covered intent is
                    // already represented by its discovered root row.
                    let retained: i64 = transaction.query_row(
                        "SELECT
                           (SELECT COUNT(*) FROM sessions
                            WHERE account_id = ?1 AND parent_id IS NULL AND final = 0) +
                           (SELECT COUNT(*) FROM session_create_intents
                            WHERE account_id = ?1 AND covered_session_id IS NULL)",
                        params![&account_id],
                        |record| record.get(0),
                    )?;
                    if retained >= max_retained_roots.max(1) {
                        transaction.commit()?;
                        return Ok(SessionCreateIntent::RetainedRootLimit);
                    }
                    transaction.execute(
                        "INSERT INTO session_create_intents
                           (account_id, request_key_hash, request_hash, state,
                            covered_session_id, created_ms, updated_ms)
                         VALUES (?1, ?2, ?3, 'dispatching', NULL, ?4, ?4)",
                        params![account_id, request_key_hash, request_hash, now_ms],
                    )?;
                    SessionCreateIntent::Created
                }
            };
            transaction.commit()?;
            Ok(outcome)
        })
        .await
    }

    /// Exact singleton-control accounting for the hosted retained-root quota. This is diagnostic;
    /// create admission performs the same expression inside its insert transaction.
    pub async fn retained_root_slots(&self, account_id: String) -> Result<i64> {
        self.call(move |connection| {
            connection.query_row(
                "SELECT
                   (SELECT COUNT(*) FROM sessions
                    WHERE account_id = ?1 AND parent_id IS NULL AND final = 0) +
                   (SELECT COUNT(*) FROM session_create_intents
                    WHERE account_id = ?1 AND covered_session_id IS NULL)",
                params![account_id],
                |record| record.get(0),
            )
        })
        .await
    }

    /// A transport, redirect, server response, or process restart cannot prove non-commit. Mark
    /// the durable slot explicitly for operators/tests; both states count identically for safety.
    pub async fn mark_session_create_uncertain(
        &self,
        account_id: String,
        request_key_hash: String,
        request_hash: String,
        now_ms: i64,
    ) -> Result<()> {
        self.call(move |connection| {
            connection.execute(
                "UPDATE session_create_intents SET state = 'uncertain', updated_ms = ?4
                 WHERE account_id = ?1 AND request_key_hash = ?2 AND request_hash = ?3",
                params![account_id, request_key_hash, request_hash, now_ms],
            )?;
            Ok(())
        })
        .await
    }

    /// Release only a definitive pre-commit rejection of the exact request. Ambiguous slots must
    /// never use this path.
    pub async fn abandon_session_create_intent(
        &self,
        account_id: String,
        request_key_hash: String,
        request_hash: String,
    ) -> Result<()> {
        self.call(move |connection| {
            connection.execute(
                "DELETE FROM session_create_intents
                 WHERE account_id = ?1 AND request_key_hash = ?2 AND request_hash = ?3",
                params![account_id, request_key_hash, request_hash],
            )?;
            Ok(())
        })
        .await
    }

    /// Durable intents not already conservatively paired with an observed root are additional
    /// possible root sessions. Pairing is one-to-one and only reduces double-counting; every
    /// observed root remains counted independently in `sessions`.
    pub async fn uncovered_session_create_intents(&self, account_id: String) -> Result<i64> {
        self.call(move |connection| {
            connection.query_row(
                "SELECT COUNT(*) FROM session_create_intents
                 WHERE account_id = ?1 AND covered_session_id IS NULL",
                params![account_id],
                |record| record.get(0),
            )
        })
        .await
    }

    /// Atomically install the owned projection and its durable idempotency mapping. The boolean
    /// is true exactly once for a newly inserted session, which is the only response that may
    /// convert a pending create reservation into a live-session increment.
    pub async fn record_created_session(
        &self,
        row: SessionRow,
        create: Option<(String, String, i64)>,
    ) -> Result<bool> {
        self.call(move |connection| {
            let transaction = connection.transaction()?;
            let inserted = insert_session_record(&transaction, &row)?;
            if let Some((request_key_hash, request_hash, created_ms)) = create {
                let existing = transaction
                    .query_row(
                        "SELECT request_hash, session_id FROM session_create_requests
                         WHERE account_id = ?1 AND request_key_hash = ?2",
                        params![&row.account_id, &request_key_hash],
                        |record| Ok((record.get::<_, String>(0)?, record.get::<_, String>(1)?)),
                    )
                    .optional()?;
                match existing {
                    Some((existing_hash, existing_session))
                        if existing_hash == request_hash && existing_session == row.id => {}
                    Some(_) => {
                        return Err(rusqlite::Error::InvalidParameterName(
                            "create idempotency identity was reused".into(),
                        ));
                    }
                    None => {
                        transaction.execute(
                            "INSERT INTO session_create_requests
                               (account_id, request_key_hash, request_hash, session_id, created_ms)
                             VALUES (?1, ?2, ?3, ?4, ?5)",
                            params![
                                &row.account_id,
                                request_key_hash,
                                request_hash,
                                &row.id,
                                created_ms
                            ],
                        )?;
                    }
                }
                transaction.execute(
                    "DELETE FROM session_create_intents
                     WHERE account_id = ?1 AND request_key_hash = ?2 AND request_hash = ?3",
                    params![&row.account_id, request_key_hash, request_hash],
                )?;
            }
            transaction.commit()?;
            Ok(inserted)
        })
        .await
    }

    /// Insert Brain-discovered sessions as one transaction. Existing meter folds are never
    /// replaced; only authoritative tree identity and immutable compute shape are refreshed. A
    /// cross-tenant ID collision aborts the whole batch.
    pub async fn upsert_discovered_sessions(&self, rows: Vec<SessionRow>) -> Result<()> {
        self.call(move |connection| {
            let transaction = connection.transaction()?;
            for row in rows {
                if let Some(owner) = transaction
                    .query_row(
                        "SELECT account_id FROM sessions WHERE id = ?1",
                        params![&row.id],
                        |record| record.get::<_, String>(0),
                    )
                    .optional()?
                {
                    if owner != row.account_id {
                        return Err(rusqlite::Error::InvalidParameterName(
                            "discovered session id belongs to another account".into(),
                        ));
                    }
                    transaction.execute(
                        "UPDATE sessions SET parent_id = ?2, root_id = ?3, depth = ?4, shape = ?5
                         WHERE id = ?1",
                        params![&row.id, &row.parent_id, &row.root_id, row.depth, &row.shape],
                    )?;
                    continue;
                }
                transaction.execute(
                    "INSERT INTO sessions
                     (id, account_id, key_id, parent_id, root_id, depth, shape, created_ms, final,
                      folded_seq, running_ms, turn_open_ms, session_storage_byte_s,
                      session_storage_byte_ms_remainder, web_search_queries,
                      storage_transition_ms, metered_to_ms, session_storage_bytes,
                      upload_reserved_bytes, session_state)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                             ?14, ?15, ?16, ?17, ?18, ?19, ?20)",
                    params![
                        &row.id,
                        &row.account_id,
                        &row.key_id,
                        &row.parent_id,
                        &row.root_id,
                        row.depth,
                        &row.shape,
                        row.created_ms,
                        row.is_final as i64,
                        row.fold.folded_seq,
                        row.fold.running_ms,
                        row.fold.turn_open_ms,
                        row.fold.session_storage_byte_seconds,
                        row.fold.session_storage_byte_millisecond_remainder,
                        row.fold.web_search_queries,
                        row.fold.storage_transition_ms,
                        row.fold.metered_to_ms,
                        row.fold.session_storage_bytes,
                        row.fold.upload_reserved_bytes,
                        row.fold.session_state,
                    ],
                )?;
                if row.parent_id.is_none() {
                    // A root first observed through strong follow-up of tenant discovery may be
                    // the result of a create whose HTTP response was lost. Pair it with one
                    // unresolved intent so the root and possible commit are not double-counted.
                    // This is only a counting cover, never an idempotency mapping: an identical
                    // retry still asks Brain and proves its exact session identity.
                    transaction.execute(
                        "UPDATE session_create_intents
                         SET covered_session_id = ?2, state = 'uncertain',
                             updated_ms = MAX(updated_ms, ?3)
                         WHERE account_id = ?1 AND request_key_hash = (
                           SELECT request_key_hash FROM session_create_intents
                           WHERE account_id = ?1 AND covered_session_id IS NULL
                           ORDER BY created_ms, request_key_hash LIMIT 1
                         )",
                        params![&row.account_id, &row.id, row.created_ms],
                    )?;
                }
            }
            transaction.commit()
        })
        .await
    }

    /// Register an account for background Brain-index discovery before the first create call.
    /// This closes the commit/response crash gap without scanning accounts that never use Brain.
    pub async fn enable_session_discovery(&self, account_id: String) -> Result<()> {
        self.call(move |connection| {
            connection.execute(
                "INSERT OR IGNORE INTO session_discovery (account_id, watermark_ms)
                 VALUES (?1, 0)",
                params![account_id],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn discovery_watermark(&self, account_id: String) -> Result<i64> {
        self.enable_session_discovery(account_id.clone()).await?;
        self.call(move |connection| {
            connection.query_row(
                "SELECT watermark_ms FROM session_discovery WHERE account_id = ?1",
                params![account_id],
                |record| record.get(0),
            )
        })
        .await
    }

    /// Successful discoveries advance monotonically; failed sweeps deliberately never call this.
    pub async fn advance_discovery_watermark(
        &self,
        account_id: String,
        cutoff_ms: i64,
    ) -> Result<()> {
        self.call(move |connection| {
            connection.execute(
                "UPDATE session_discovery
                 SET watermark_ms = MAX(watermark_ms, ?2)
                 WHERE account_id = ?1",
                params![account_id, cutoff_ms],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn discovery_accounts(&self) -> Result<Vec<String>> {
        self.call(move |connection| {
            let mut statement = connection
                .prepare("SELECT account_id FROM session_discovery ORDER BY account_id")?;
            statement.query_map([], |record| record.get(0))?.collect()
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

    /// Batch lookup for one bounded changed window. Chunking stays below SQLite's portable bind
    /// limit and uses one blocking-pool hop instead of one hop per discovered child.
    pub async fn sessions_by_ids(
        &self,
        account_id: String,
        ids: Vec<String>,
    ) -> Result<Vec<SessionRow>> {
        self.call(move |connection| {
            let mut rows = Vec::with_capacity(ids.len());
            for chunk in ids.chunks(500) {
                let placeholders = std::iter::repeat_n("?", chunk.len())
                    .collect::<Vec<_>>()
                    .join(",");
                let mut statement = connection.prepare(&format!(
                    "{SESSION_COLS} WHERE account_id = ? AND id IN ({placeholders})"
                ))?;
                rows.extend(
                    statement
                        .query_map(
                            rusqlite::params_from_iter(
                                std::iter::once(&account_id).chain(chunk.iter()),
                            ),
                            session_row,
                        )?
                        .collect::<rusqlite::Result<Vec<_>>>()?,
                );
            }
            Ok(rows)
        })
        .await
    }

    /// Locally known turns whose running estimate must advance even when the remote HEAD has not
    /// changed since admission. This is bounded by the product's concurrent-turn policy.
    pub async fn open_turn_sessions(&self, account_id: String) -> Result<Vec<SessionRow>> {
        self.call(move |connection| {
            let mut statement = connection.prepare(&format!(
                "{SESSION_COLS} WHERE account_id = ?1 AND final = 0
                 AND turn_open_ms IS NOT NULL ORDER BY turn_open_ms, id"
            ))?;
            statement
                .query_map(params![account_id], session_row)?
                .collect()
        })
        .await
    }

    /// Oldest non-zero byte meters due for a background integral settlement. The indexed time
    /// predicate and batch bound prevent the ordinary discovery tick from scanning every ended
    /// session merely because its durable storage remains billable.
    pub async fn due_storage_sessions(
        &self,
        account_id: String,
        due_before_ms: i64,
        limit: usize,
    ) -> Result<Vec<SessionRow>> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX).max(1);
        self.call(move |connection| {
            let mut statement = connection.prepare(&format!(
                "{SESSION_COLS} WHERE account_id = ?1 AND final = 0
                 AND metered_to_ms <= ?2
                 AND (session_storage_bytes > 0 OR upload_reserved_bytes > 0)
                 ORDER BY metered_to_ms, id LIMIT ?3"
            ))?;
            statement
                .query_map(params![account_id, due_before_ms, limit], session_row)?
                .collect()
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

    /// Resource-bearing roots count against the account cap whether or not their lazy sandbox
    /// exists. Failed and deleting are not release proofs: only a strong ended projection or a
    /// physically completed deletion frees the local floor. Durable descendants have a separate
    /// Brain-sealed tree quota.
    pub async fn live_root_count(&self, account_id: String) -> Result<i64> {
        self.call(move |c| {
            c.query_row(
                "SELECT COUNT(*) FROM sessions WHERE account_id = ?1 AND final = 0
                 AND parent_id IS NULL
                 AND session_state IN ('open', 'ending', 'failed', 'deleting')",
                params![account_id],
                |r| r.get(0),
            )
        })
        .await
    }

    pub async fn root_creates_since(&self, account_id: String, since_ms: i64) -> Result<i64> {
        self.call(move |c| {
            c.query_row(
                "SELECT COUNT(*) FROM sessions
                 WHERE account_id = ?1 AND parent_id IS NULL AND created_ms >= ?2",
                params![account_id, since_ms],
                |r| r.get(0),
            )
        })
        .await
    }

    /// Mark the selected session and every locally discovered descendant as deleting and retain
    /// one small anchor for efficient Brain-side completion checks.
    pub async fn begin_subtree_deletion(
        &self,
        account_id: String,
        session_id: String,
        accepted_ms: i64,
    ) -> Result<()> {
        self.begin_subtree_deletion_with_chain(account_id, session_id, Vec::new(), accepted_ms)
            .await
    }

    /// Atomically install a strongly hydrated ancestor chain before resolving overlap aliases.
    /// This is essential for a Brain-native descendant first seen on its DELETE request: without
    /// its previously hidden parents, a locally active ancestor purge would be invisible and the
    /// two jobs could be claimed concurrently.
    pub async fn begin_subtree_deletion_with_chain(
        &self,
        account_id: String,
        session_id: String,
        chain: Vec<SessionRow>,
        accepted_ms: i64,
    ) -> Result<()> {
        self.call(move |connection| {
            let transaction = connection.transaction()?;
            for row in &chain {
                if row.account_id != account_id {
                    return Err(rusqlite::Error::InvalidParameterName(
                        "hydrated deletion ancestor belongs to another account".into(),
                    ));
                }
                insert_session_record(&transaction, row)?;
                let identity = transaction.query_row(
                    "SELECT account_id, parent_id, root_id, depth FROM sessions WHERE id = ?1",
                    params![&row.id],
                    |record| {
                        Ok((
                            record.get::<_, String>(0)?,
                            record.get::<_, Option<String>>(1)?,
                            record.get::<_, String>(2)?,
                            record.get::<_, i64>(3)?,
                        ))
                    },
                )?;
                if identity
                    != (
                        row.account_id.clone(),
                        row.parent_id.clone(),
                        row.root_id.clone(),
                        row.depth,
                    )
                {
                    return Err(rusqlite::Error::InvalidParameterName(
                        "hydrated deletion ancestor contradicts persisted identity".into(),
                    ));
                }
            }
            transaction.execute(
                "WITH RECURSIVE subtree(id) AS (
                   SELECT id FROM sessions WHERE id = ?1 AND account_id = ?2
                   UNION ALL
                   SELECT child.id FROM sessions child JOIN subtree parent
                     ON child.parent_id = parent.id
                   WHERE child.account_id = ?2
                 )
                 UPDATE sessions SET session_state = 'deleting'
                 WHERE id IN (SELECT id FROM subtree) AND final = 0",
                params![&session_id, &account_id],
            )?;
            if transaction
                .query_row(
                    "SELECT 1 FROM session_deletions WHERE session_id = ?1",
                    params![&session_id],
                    |_| Ok(()),
                )
                .optional()?
                .is_some()
            {
                transaction.commit()?;
                return Ok(());
            }

            // A later descendant request is a durable status alias when an ancestor purge
            // already covers it. Resolve through an existing alias to the executable anchor.
            let ancestor_anchor = transaction
                .query_row(
                    "WITH RECURSIVE ancestors(id, parent_id, distance) AS (
                       SELECT id, parent_id, 0 FROM sessions
                        WHERE id = ?1 AND account_id = ?2
                       UNION ALL
                       SELECT parent.id, parent.parent_id, child.distance + 1
                         FROM sessions parent JOIN ancestors child ON parent.id = child.parent_id
                        WHERE parent.account_id = ?2
                     )
                     SELECT deletion.anchor_id, anchor.phase, anchor.completed_ms
                       FROM ancestors
                       JOIN session_deletions deletion ON deletion.session_id = ancestors.id
                       JOIN session_deletions anchor ON anchor.session_id = deletion.anchor_id
                      WHERE ancestors.id != ?1
                      ORDER BY ancestors.distance
                      LIMIT 1",
                    params![&session_id, &account_id],
                    |record| {
                        Ok((
                            record.get::<_, String>(0)?,
                            record.get::<_, String>(1)?,
                            record.get::<_, Option<i64>>(2)?,
                        ))
                    },
                )
                .optional()?;
            let (anchor_id, phase, completed_ms) = match ancestor_anchor {
                Some((anchor_id, phase, completed_ms)) if phase == "succeeded" => {
                    (anchor_id, "succeeded", completed_ms)
                }
                Some((anchor_id, _, _)) => (anchor_id, "ending", None),
                None => (session_id.clone(), "ending", None),
            };
            transaction.execute(
                "INSERT INTO session_deletions
                   (session_id, account_id, anchor_id, phase, accepted_ms, updated_ms,
                    completed_ms, next_attempt_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6, ?5)",
                params![
                    &session_id,
                    &account_id,
                    &anchor_id,
                    phase,
                    accepted_ms,
                    completed_ms
                ],
            )?;

            if anchor_id == session_id && phase != "succeeded" {
                // A newly accepted ancestor waits behind every already-active descendant anchor.
                // This also covers descendants that were claimed immediately before acceptance.
                let dependencies = {
                    let mut statement = transaction.prepare(
                        "WITH RECURSIVE subtree(id) AS (
                           SELECT id FROM sessions WHERE id = ?1 AND account_id = ?2
                           UNION ALL
                           SELECT child.id FROM sessions child JOIN subtree parent
                             ON child.parent_id = parent.id
                            WHERE child.account_id = ?2
                         )
                         SELECT DISTINCT deletion.anchor_id
                           FROM subtree
                           JOIN session_deletions deletion ON deletion.session_id = subtree.id
                           JOIN session_deletions anchor ON anchor.session_id = deletion.anchor_id
                          WHERE subtree.id != ?1 AND anchor.phase != 'succeeded'
                            AND deletion.anchor_id != ?1",
                    )?;
                    statement
                        .query_map(params![&session_id, &account_id], |record| {
                            record.get::<_, String>(0)
                        })?
                        .collect::<rusqlite::Result<Vec<_>>>()?
                };
                for dependency_id in dependencies {
                    transaction.execute(
                        "INSERT OR IGNORE INTO session_deletion_dependencies
                           (session_id, dependency_id) VALUES (?1, ?2)",
                        params![&session_id, dependency_id],
                    )?;
                }
            }
            transaction.commit()
        })
        .await
    }

    pub async fn deletion_roots(&self, account_id: String) -> Result<Vec<String>> {
        self.call(move |connection| {
            let mut statement = connection.prepare(
                "SELECT session_id FROM session_deletions
                 WHERE account_id = ?1 AND phase != 'succeeded'
                 ORDER BY accepted_ms, session_id",
            )?;
            statement
                .query_map(params![account_id], |record| record.get(0))?
                .collect()
        })
        .await
    }

    pub async fn deletion(&self, session_id: String) -> Result<Option<DeletionRow>> {
        self.call(move |connection| {
            connection
                .query_row(
                    &format!("{DELETION_COLS} FROM session_deletions WHERE session_id = ?1"),
                    params![session_id],
                    deletion_row,
                )
                .optional()
        })
        .await
    }

    /// Oldest retryable jobs. The singleton worker may repeat a row after a crash; every phase is
    /// idempotent and advances only after the corresponding remote/durable boundary succeeds.
    pub async fn pending_deletions(&self, limit: usize) -> Result<Vec<DeletionRow>> {
        self.call(move |connection| {
            let mut statement = connection.prepare(&format!(
                "{DELETION_COLS} FROM session_deletions WHERE phase != 'succeeded'
                 ORDER BY updated_ms, accepted_ms, session_id LIMIT ?1"
            ))?;
            statement
                .query_map(
                    params![i64::try_from(limit).unwrap_or(i64::MAX)],
                    deletion_row,
                )?
                .collect()
        })
        .await
    }

    /// Recover all scheduler leases on singleton startup. Platform stops the old task before
    /// starting its replacement, so no live worker can still own these process-local claims.
    pub async fn clear_deletion_claims(&self) -> Result<()> {
        self.call(move |connection| {
            connection.execute(
                "UPDATE session_deletions SET claimed_ms = NULL
                 WHERE phase != 'succeeded' AND claimed_ms IS NOT NULL",
                [],
            )?;
            Ok(())
        })
        .await
    }

    /// Claim one non-overlapping retry batch atomically. Aliases never execute, dependencies must
    /// have completed, and no selected ancestor/descendant pair can race its Brain purge. Siblings
    /// remain independent and may run together.
    pub async fn claim_pending_deletions(
        &self,
        limit: usize,
        claimed_ms: i64,
    ) -> Result<Vec<DeletionRow>> {
        self.call(move |connection| {
            let transaction = connection.transaction()?;
            let scan_limit =
                i64::try_from(limit.max(1).saturating_mul(16).min(4096)).unwrap_or(i64::MAX);
            let candidates = {
                let mut statement = transaction.prepare(&format!(
                    "{DELETION_COLS} FROM session_deletions job
                     WHERE job.phase != 'succeeded' AND job.claimed_ms IS NULL
                       AND job.next_attempt_ms <= ?2
                       AND job.anchor_id = job.session_id
                       AND NOT EXISTS (
                         SELECT 1 FROM session_deletion_dependencies dependency
                         JOIN session_deletions prerequisite
                           ON prerequisite.session_id = dependency.dependency_id
                         WHERE dependency.session_id = job.session_id
                           AND prerequisite.phase != 'succeeded'
                       )
                     ORDER BY job.updated_ms, job.accepted_ms, job.session_id LIMIT ?1"
                ))?;
                statement
                    .query_map(params![scan_limit, claimed_ms], deletion_row)?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            };
            let mut selected: Vec<DeletionRow> = Vec::with_capacity(limit);
            for mut candidate in candidates {
                if selected.len() >= limit {
                    break;
                }
                let mut overlaps = false;
                for claimed in &selected {
                    if claimed.account_id != candidate.account_id {
                        continue;
                    }
                    let related: i64 = transaction.query_row(
                        "WITH RECURSIVE candidate_tree(id) AS (
                           SELECT id FROM sessions WHERE id = ?1 AND account_id = ?3
                           UNION ALL
                           SELECT child.id FROM sessions child JOIN candidate_tree parent
                             ON child.parent_id = parent.id WHERE child.account_id = ?3
                         ), claimed_tree(id) AS (
                           SELECT id FROM sessions WHERE id = ?2 AND account_id = ?3
                           UNION ALL
                           SELECT child.id FROM sessions child JOIN claimed_tree parent
                             ON child.parent_id = parent.id WHERE child.account_id = ?3
                         )
                         SELECT EXISTS(SELECT 1 FROM candidate_tree WHERE id = ?2)
                              OR EXISTS(SELECT 1 FROM claimed_tree WHERE id = ?1)",
                        params![
                            &candidate.session_id,
                            &claimed.session_id,
                            &candidate.account_id
                        ],
                        |record| record.get(0),
                    )?;
                    if related != 0 {
                        overlaps = true;
                        break;
                    }
                }
                if overlaps {
                    continue;
                }
                let changed = transaction.execute(
                    "UPDATE session_deletions SET claimed_ms = ?2
                     WHERE session_id = ?1 AND phase != 'succeeded' AND claimed_ms IS NULL",
                    params![&candidate.session_id, claimed_ms],
                )?;
                if changed == 1 {
                    candidate.claimed_ms = Some(claimed_ms);
                    selected.push(candidate);
                }
            }
            transaction.commit()?;
            Ok(selected)
        })
        .await
    }

    pub async fn release_deletion_claim(&self, session_id: String) -> Result<()> {
        self.call(move |connection| {
            connection.execute(
                "UPDATE session_deletions SET claimed_ms = NULL WHERE session_id = ?1",
                params![session_id],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn set_deletion_phase(
        &self,
        session_id: String,
        phase: String,
        updated_ms: i64,
    ) -> Result<()> {
        self.call(move |connection| {
            let changed = connection.execute(
                "UPDATE session_deletions
                 SET phase = ?2, updated_ms = ?3, last_error = NULL, attempts = 0,
                     next_attempt_ms = ?3
                 WHERE session_id = ?1 AND phase != 'succeeded'",
                params![session_id, phase, updated_ms],
            )?;
            if changed == 0 {
                return Err(rusqlite::Error::QueryReturnedNoRows);
            }
            Ok(())
        })
        .await
    }

    pub async fn record_deletion_error(
        &self,
        session_id: String,
        error: String,
        updated_ms: i64,
    ) -> Result<()> {
        self.call(move |connection| {
            let transaction = connection.transaction()?;
            let attempts = transaction
                .query_row(
                    "SELECT attempts FROM session_deletions
                     WHERE session_id = ?1 AND phase != 'succeeded'",
                    params![&session_id],
                    |record| record.get::<_, i64>(0),
                )
                .optional()?;
            let Some(attempts) = attempts else {
                transaction.commit()?;
                return Ok(());
            };
            let next_attempt = attempts.saturating_add(1);
            let retry_at_ms = updated_ms.saturating_add(deletion_retry_delay_ms(
                &session_id,
                u32::try_from(next_attempt).unwrap_or(u32::MAX),
            ));
            transaction.execute(
                "UPDATE session_deletions
                 SET updated_ms = ?2, last_error = ?3, attempts = ?4, next_attempt_ms = ?5
                 WHERE session_id = ?1 AND phase != 'succeeded'",
                params![
                    &session_id,
                    updated_ms,
                    error.chars().take(512).collect::<String>(),
                    next_attempt,
                    retry_at_ms,
                ],
            )?;
            transaction.commit()
        })
        .await
    }

    pub async fn subtree_sessions(
        &self,
        account_id: String,
        session_id: String,
    ) -> Result<Vec<SessionRow>> {
        self.call(move |connection| {
            let mut statement = connection.prepare(&format!(
                "{SESSION_COLS} WHERE account_id = ?2 AND id IN (
                   WITH RECURSIVE subtree(id) AS (
                     SELECT id FROM sessions WHERE id = ?1 AND account_id = ?2
                     UNION ALL
                     SELECT child.id FROM sessions child JOIN subtree parent
                       ON child.parent_id = parent.id
                     WHERE child.account_id = ?2
                   )
                   SELECT id FROM subtree
                 ) ORDER BY created_ms DESC"
            ))?;
            statement
                .query_map(params![session_id, account_id], session_row)?
                .collect()
        })
        .await
    }

    /// Complete every selected deletion anchor in the local subtree. Keeping the small tombstone
    /// makes a lost strict-delete response observable without retaining Brain/S3 content.
    pub async fn finish_subtree_deletion(
        &self,
        account_id: String,
        session_id: String,
    ) -> Result<()> {
        self.call(move |connection| {
            let transaction = connection.transaction()?;
            // Billing folds and the small session/deletion tombstones are retained, but cached
            // model-visible Tool payloads are not part of the ledger. Purge them atomically once
            // Brain confirms the physical subtree deletion; the alpha promises no content
            // retention after `session.delete()` completes.
            for table in [
                "external_tool_calls",
                "hosted_tool_calls",
                "output_requests",
            ] {
                transaction.execute(
                    &format!(
                        "WITH RECURSIVE subtree(id) AS (
                           SELECT id FROM sessions WHERE id = ?1 AND account_id = ?2
                           UNION ALL
                           SELECT child.id FROM sessions child JOIN subtree parent
                             ON child.parent_id = parent.id
                           WHERE child.account_id = ?2
                         )
                         DELETE FROM {table} WHERE session_id IN (SELECT id FROM subtree)"
                    ),
                    params![&session_id, &account_id],
                )?;
            }
            // A discovery-covered ambiguous create is already represented by its root row while
            // retained. Once Brain confirms that subtree physically gone, neither representation
            // may continue consuming a retained-root slot.
            transaction.execute(
                "DELETE FROM session_create_intents
                 WHERE account_id = ?2 AND covered_session_id IN (
                   WITH RECURSIVE subtree(id) AS (
                     SELECT id FROM sessions WHERE id = ?1 AND account_id = ?2
                     UNION ALL
                     SELECT child.id FROM sessions child JOIN subtree parent
                       ON child.parent_id = parent.id
                     WHERE child.account_id = ?2
                   )
                   SELECT id FROM subtree
                 )",
                params![&session_id, &account_id],
            )?;
            transaction.execute(
                "UPDATE session_deletions
                 SET phase = 'succeeded', updated_ms = ?3, completed_ms = ?3,
                     last_error = NULL, claimed_ms = NULL, attempts = 0,
                     next_attempt_ms = ?3
                 WHERE account_id = ?2 AND session_id IN (
                   WITH RECURSIVE subtree(id) AS (
                     SELECT id FROM sessions WHERE id = ?1 AND account_id = ?2
                     UNION ALL
                     SELECT child.id FROM sessions child JOIN subtree parent
                       ON child.parent_id = parent.id
                     WHERE child.account_id = ?2
                   )
                   SELECT id FROM subtree
                 )",
                params![&session_id, &account_id, crate::now_ms()],
            )?;
            transaction.commit()
        })
        .await
    }

    // ---- typed message output --------------------------------------------------------------

    pub async fn begin_output(&self, row: OutputRequestRow) -> Result<BeginOutput> {
        self.call(move |connection| {
            let transaction = connection.transaction()?;
            if let Some(key_hash) = row.idempotency_key_hash.as_ref()
                && let Some(existing) = transaction
                    .query_row(
                        &format!(
                            "{OUTPUT_COLS} WHERE session_id = ?1 AND idempotency_key_hash = ?2"
                        ),
                        params![row.session_id, key_hash],
                        output_request_row,
                    )
                    .optional()?
            {
                transaction.commit()?;
                return Ok(if existing.request_hash == row.request_hash {
                    BeginOutput::Existing(existing)
                } else {
                    BeginOutput::RequestMismatch
                });
            }
            transaction.execute(
                "INSERT INTO output_requests
                 (id, session_id, schema_hash, schema_json, max_attempts, attempts, status,
                  turn_id, accepted_json, result_json, error_json, idempotency_key_hash,
                  request_hash, created_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                params![
                    row.id,
                    row.session_id,
                    row.schema_hash,
                    row.schema_json,
                    row.max_attempts,
                    row.attempts,
                    row.status,
                    row.turn_id,
                    row.accepted_json,
                    row.result_json,
                    row.error_json,
                    row.idempotency_key_hash,
                    row.request_hash,
                    row.created_ms,
                ],
            )?;
            transaction.commit()?;
            Ok(BeginOutput::Created(row))
        })
        .await
    }

    pub async fn output_request(&self, id: String) -> Result<Option<OutputRequestRow>> {
        self.call(move |connection| {
            connection
                .query_row(
                    &format!("{OUTPUT_COLS} WHERE id = ?1"),
                    params![id],
                    output_request_row,
                )
                .optional()
        })
        .await
    }

    pub async fn accept_output(
        &self,
        id: String,
        turn_id: String,
        accepted_json: String,
    ) -> Result<()> {
        let updated = self
            .call(move |connection| {
                let updated = connection.execute(
                    "UPDATE output_requests SET turn_id = ?2, accepted_json = ?3
                 WHERE id = ?1 AND (turn_id IS NULL OR turn_id = ?2)",
                    params![id, turn_id, accepted_json],
                )?;
                Ok(updated == 1)
            })
            .await?;
        if updated {
            Ok(())
        } else {
            Err(Error::Conflict(
                "output request is bound to a different Brain turn".into(),
            ))
        }
    }

    /// Bind the authenticated executor delivery to the Brain turn that carried the reserved
    /// message metadata. The executor can race the HTTP 202 response, so first delivery may claim
    /// an unset turn; every subsequent writer must present the same identity.
    pub async fn claim_output_turn(
        &self,
        id: String,
        session_id: String,
        turn_id: String,
    ) -> Result<bool> {
        self.call(move |connection| {
            let updated = connection.execute(
                "UPDATE output_requests SET turn_id = ?3
                 WHERE id = ?1 AND session_id = ?2 AND (turn_id IS NULL OR turn_id = ?3)",
                params![id, session_id, turn_id],
            )?;
            Ok(updated == 1)
        })
        .await
    }

    /// Remove an output identity whose message never reached Brain admission. Once accepted or
    /// attempted, it is durable and must remain available to executor replay.
    pub async fn abandon_output(&self, id: String) -> Result<()> {
        self.call(move |connection| {
            connection.execute(
                "DELETE FROM output_requests
                 WHERE id = ?1 AND accepted_json IS NULL AND attempts = 0",
                params![id],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn external_call_response(
        &self,
        session_id: String,
        call_id: String,
    ) -> Result<Option<String>> {
        self.call(move |connection| {
            connection
                .query_row(
                    "SELECT response_json FROM external_tool_calls
                     WHERE session_id = ?1 AND call_id = ?2",
                    params![session_id, call_id],
                    |row| row.get(0),
                )
                .optional()
        })
        .await
    }

    /// Exact-result cache for replay-safe hosted capabilities such as managed web reads. The
    /// request hash prevents a reused call identity from changing its logical operation.
    pub async fn hosted_tool_response(
        &self,
        session_id: String,
        call_id: String,
        request_hash: String,
    ) -> Result<Option<String>> {
        let row = self
            .call(move |connection| {
                connection
                    .query_row(
                        "SELECT request_hash, response_json FROM hosted_tool_calls
                     WHERE session_id = ?1 AND call_id = ?2",
                        params![session_id, call_id],
                        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                    )
                    .optional()
            })
            .await?;
        match row {
            Some((stored_hash, _)) if stored_hash != request_hash => Err(Error::Conflict(
                "hosted Tool call identity was reused with a different request".into(),
            )),
            Some((_, response)) => Ok(Some(response)),
            None => Ok(None),
        }
    }

    /// Store or race-replay one hosted result. Every concurrent caller returns the first committed
    /// response, so Brain observes one stable logical result even if a transport retry overlaps.
    pub async fn record_hosted_tool_response(
        &self,
        session_id: String,
        call_id: String,
        request_hash: String,
        response_json: String,
        now_ms: i64,
    ) -> Result<String> {
        let expected_hash = request_hash.clone();
        let (stored_hash, stored_response) = self
            .call(move |connection| {
                let transaction = connection.transaction()?;
                transaction.execute(
                    "INSERT OR IGNORE INTO hosted_tool_calls
                 (session_id, call_id, request_hash, response_json, created_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![session_id, call_id, request_hash, response_json, now_ms],
                )?;
                let stored = transaction.query_row(
                    "SELECT request_hash, response_json FROM hosted_tool_calls
                 WHERE session_id = ?1 AND call_id = ?2",
                    params![session_id, call_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )?;
                transaction.commit()?;
                Ok(stored)
            })
            .await?;
        if stored_hash != expected_hash {
            return Err(Error::Conflict(
                "hosted Tool call identity was reused with a different request".into(),
            ));
        }
        Ok(stored_response)
    }

    /// Journal one executor decision and cache the exact response in the same SQLite
    /// transaction. A replay with the same `(session_id, call_id)` returns the first response.
    #[allow(clippy::too_many_arguments)]
    pub async fn record_external_attempt(
        &self,
        session_id: String,
        call_id: String,
        output_id: String,
        response_json: String,
        status: String,
        result_json: Option<String>,
        error_json: Option<String>,
        expected_attempts: i64,
        now_ms: i64,
    ) -> Result<String> {
        let response = self
            .call(move |connection| {
                let transaction = connection.transaction()?;
                if let Some(existing) = transaction
                    .query_row(
                        "SELECT response_json FROM external_tool_calls
                     WHERE session_id = ?1 AND call_id = ?2",
                        params![session_id, call_id],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()?
                {
                    transaction.commit()?;
                    return Ok(Some(existing));
                }
                let updated = transaction.execute(
                    "UPDATE output_requests
                 SET attempts = attempts + 1, status = ?2, result_json = ?3, error_json = ?4
                 WHERE id = ?1 AND session_id = ?5 AND status = 'pending' AND attempts = ?6",
                    params![
                        output_id,
                        status,
                        result_json,
                        error_json,
                        session_id,
                        expected_attempts
                    ],
                )?;
                if updated != 1 {
                    transaction.commit()?;
                    return Ok(None);
                }
                transaction.execute(
                    "INSERT INTO external_tool_calls
                 (session_id, call_id, output_id, response_json, created_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![session_id, call_id, output_id, response_json, now_ms],
                )?;
                transaction.commit()?;
                Ok(Some(response_json))
            })
            .await?;
        response.ok_or_else(|| {
            Error::Conflict("structured-output attempt lost a concurrent terminal race".into())
        })
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
                    session_storage_byte_s = ?5, session_storage_byte_ms_remainder = ?6,
                    web_search_queries = ?7, storage_transition_ms = ?8, metered_to_ms = ?9,
                    session_storage_bytes = ?10, upload_reserved_bytes = ?11,
                    session_state = ?12, final = ?13
                 WHERE id = ?1 AND folded_seq <= ?2 AND metered_to_ms <= ?9 AND final = 0",
                params![
                    session_id,
                    fold.folded_seq,
                    fold.running_ms,
                    fold.turn_open_ms,
                    fold.session_storage_byte_seconds,
                    fold.session_storage_byte_millisecond_remainder,
                    fold.web_search_queries,
                    fold.storage_transition_ms,
                    fold.metered_to_ms,
                    fold.session_storage_bytes,
                    fold.upload_reserved_bytes,
                    fold.session_state,
                    is_final as i64,
                ],
            )?;
            if n > 0 {
                tx.execute(
                    "INSERT INTO ledger (ref, account_id, microusd, updated_ms)
                     VALUES ('usage:' || ?1, ?2, ?3, ?4)
                     ON CONFLICT(ref) DO UPDATE SET microusd = ?3, updated_ms = ?4
                     WHERE excluded.updated_ms >= ledger.updated_ms",
                    params![session_id, account_id, -usage_microusd, now_ms],
                )?;
            }
            tx.commit()?;
            Ok(n > 0)
        })
        .await
    }
}

const SESSION_COLS: &str =
    "SELECT id, account_id, key_id, parent_id, root_id, depth, shape, created_ms, final,
    folded_seq, running_ms, turn_open_ms, session_storage_byte_s,
    session_storage_byte_ms_remainder, web_search_queries, storage_transition_ms,
    metered_to_ms, session_storage_bytes, upload_reserved_bytes, session_state
    FROM sessions";

const DELETION_COLS: &str = "SELECT session_id, account_id, anchor_id, phase, accepted_ms,
    updated_ms, completed_ms, last_error, claimed_ms, attempts, next_attempt_ms";

const REFUND_COLS: &str = "SELECT id, request_key, topup_id, account_id, amount_cents, status,
    provider_ref, failure_reason, created_ms, updated_ms FROM refunds";

const CREDIT_GRANT_COLS: &str = "SELECT g.id, g.request_key, g.account_id, a.email,
    g.amount_cents, g.reason, g.created_ms
    FROM credit_grants g JOIN accounts a ON a.id = g.account_id";

const OUTPUT_COLS: &str = "SELECT id, session_id, schema_hash, schema_json, max_attempts,
    attempts, status, turn_id, accepted_json, result_json, error_json, idempotency_key_hash,
    request_hash, created_ms FROM output_requests";

fn waitlist_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<WaitlistRow> {
    Ok(WaitlistRow {
        email: r.get(0)?,
        status: r.get(1)?,
        created_ms: r.get(2)?,
        invited_ms: r.get(3)?,
        joined_ms: r.get(4)?,
    })
}

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

fn refund_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<RefundRow> {
    Ok(RefundRow {
        id: r.get(0)?,
        request_key: r.get(1)?,
        topup_id: r.get(2)?,
        account_id: r.get(3)?,
        amount_cents: r.get(4)?,
        status: r.get(5)?,
        provider_ref: r.get(6)?,
        failure_reason: r.get(7)?,
        created_ms: r.get(8)?,
        updated_ms: r.get(9)?,
    })
}

fn credit_grant_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<CreditGrantRow> {
    Ok(CreditGrantRow {
        id: r.get(0)?,
        request_key: r.get(1)?,
        account_id: r.get(2)?,
        email: r.get(3)?,
        amount_cents: r.get(4)?,
        reason: r.get(5)?,
        created_ms: r.get(6)?,
    })
}

fn insert_session_record(
    transaction: &rusqlite::Transaction<'_>,
    row: &SessionRow,
) -> rusqlite::Result<bool> {
    let inserted = transaction.execute(
        "INSERT OR IGNORE INTO sessions
         (id, account_id, key_id, parent_id, root_id, depth, shape, created_ms, final,
          folded_seq, running_ms, turn_open_ms, session_storage_byte_s,
          session_storage_byte_ms_remainder, web_search_queries, storage_transition_ms,
          metered_to_ms, session_storage_bytes, upload_reserved_bytes, session_state)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                 ?14, ?15, ?16, ?17, ?18, ?19, ?20)",
        params![
            &row.id,
            &row.account_id,
            &row.key_id,
            &row.parent_id,
            &row.root_id,
            row.depth,
            &row.shape,
            row.created_ms,
            row.is_final as i64,
            row.fold.folded_seq,
            row.fold.running_ms,
            row.fold.turn_open_ms,
            row.fold.session_storage_byte_seconds,
            row.fold.session_storage_byte_millisecond_remainder,
            row.fold.web_search_queries,
            row.fold.storage_transition_ms,
            row.fold.metered_to_ms,
            row.fold.session_storage_bytes,
            row.fold.upload_reserved_bytes,
            row.fold.session_state,
        ],
    )?;
    if inserted == 0 {
        let owner: String = transaction.query_row(
            "SELECT account_id FROM sessions WHERE id = ?1",
            params![&row.id],
            |record| record.get(0),
        )?;
        if owner != row.account_id {
            return Err(rusqlite::Error::InvalidParameterName(
                "session id belongs to another account".into(),
            ));
        }
    }
    Ok(inserted != 0)
}

fn session_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRow> {
    Ok(SessionRow {
        id: r.get(0)?,
        account_id: r.get(1)?,
        key_id: r.get(2)?,
        parent_id: r.get(3)?,
        root_id: r.get(4)?,
        depth: r.get(5)?,
        shape: r.get(6)?,
        created_ms: r.get(7)?,
        is_final: r.get::<_, i64>(8)? != 0,
        fold: FoldState {
            folded_seq: r.get(9)?,
            running_ms: r.get(10)?,
            turn_open_ms: r.get(11)?,
            session_storage_byte_seconds: r.get(12)?,
            session_storage_byte_millisecond_remainder: r.get(13)?,
            web_search_queries: r.get(14)?,
            storage_transition_ms: r.get(15)?,
            metered_to_ms: r.get(16)?,
            session_storage_bytes: r.get(17)?,
            upload_reserved_bytes: r.get(18)?,
            session_state: r.get(19)?,
        },
    })
}

fn deletion_row(record: &rusqlite::Row<'_>) -> rusqlite::Result<DeletionRow> {
    Ok(DeletionRow {
        session_id: record.get(0)?,
        account_id: record.get(1)?,
        anchor_id: record.get(2)?,
        phase: record.get(3)?,
        accepted_ms: record.get(4)?,
        updated_ms: record.get(5)?,
        completed_ms: record.get(6)?,
        last_error: record.get(7)?,
        claimed_ms: record.get(8)?,
        attempts: record.get(9)?,
        next_attempt_ms: record.get(10)?,
    })
}

/// Capped exponential retry with stable per-session jitter. Persisting the resulting deadline in
/// SQLite prevents a control restart from collapsing every failed destructive job into a retry
/// storm. The one-second worker tick remains the lower scheduling resolution.
fn deletion_retry_delay_ms(session_id: &str, attempt: u32) -> i64 {
    const BASE_MS: u64 = 1_000;
    const MAX_BASE_MS: u64 = 240_000;

    let shift = attempt.saturating_sub(1).min(31);
    let base = BASE_MS.saturating_mul(1_u64 << shift).min(MAX_BASE_MS);
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in session_id
        .as_bytes()
        .iter()
        .copied()
        .chain(attempt.to_le_bytes())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let jitter = hash % (base / 5 + 1);
    i64::try_from(base + jitter).unwrap_or(i64::MAX)
}

fn output_request_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<OutputRequestRow> {
    Ok(OutputRequestRow {
        id: r.get(0)?,
        session_id: r.get(1)?,
        schema_hash: r.get(2)?,
        schema_json: r.get(3)?,
        max_attempts: r.get(4)?,
        attempts: r.get(5)?,
        status: r.get(6)?,
        turn_id: r.get(7)?,
        accepted_json: r.get(8)?,
        result_json: r.get(9)?,
        error_json: r.get(10)?,
        idempotency_key_hash: r.get(11)?,
        request_hash: r.get(12)?,
        created_ms: r.get(13)?,
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

    async fn deletion_tree() -> Db {
        let db = Db::open_memory().unwrap();
        db.create_account(
            acct("acc_delete"),
            "delete@example.com".into(),
            "h-delete".into(),
        )
        .await
        .unwrap();
        for (id, parent_id) in [
            ("ses_root", None),
            ("ses_child_a", Some("ses_root")),
            ("ses_grandchild", Some("ses_child_a")),
            ("ses_child_b", Some("ses_root")),
        ] {
            db.insert_session(SessionRow {
                id: id.into(),
                account_id: "acc_delete".into(),
                key_id: "key_delete".into(),
                parent_id: parent_id.map(str::to_owned),
                root_id: "ses_root".into(),
                depth: match id {
                    "ses_root" => 0,
                    "ses_grandchild" => 2,
                    _ => 1,
                },
                shape: "1gb".into(),
                created_ms: 5,
                is_final: false,
                fold: FoldState {
                    session_state: "open".into(),
                    ..Default::default()
                },
            })
            .await
            .unwrap();
        }
        db
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn existing_ledger_is_migrated_for_additive_session_columns() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        conn.execute("ALTER TABLE sessions DROP COLUMN web_search_queries", [])
            .unwrap();
        conn.execute("ALTER TABLE sessions DROP COLUMN upload_reserved_bytes", [])
            .unwrap();
        conn.execute(
            "ALTER TABLE sessions DROP COLUMN session_storage_byte_ms_remainder",
            [],
        )
        .unwrap();
        conn.execute("ALTER TABLE sessions DROP COLUMN storage_transition_ms", [])
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
        assert!(columns.iter().any(|name| name == "upload_reserved_bytes"));
        assert!(
            columns
                .iter()
                .any(|name| name == "session_storage_byte_ms_remainder")
        );
        assert!(columns.iter().any(|name| name == "storage_transition_ms"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn existing_deletion_jobs_gain_durable_scheduler_fields() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE session_deletions (
               session_id TEXT PRIMARY KEY,
               account_id TEXT NOT NULL,
               phase TEXT NOT NULL,
               accepted_ms INTEGER NOT NULL,
               updated_ms INTEGER NOT NULL,
               completed_ms INTEGER,
               last_error TEXT
             );
             INSERT INTO session_deletions
               (session_id, account_id, phase, accepted_ms, updated_ms)
             VALUES ('ses_old', 'acc_old', 'settling', 10, 11);",
        )
        .unwrap();

        let db = Db::init(conn).unwrap();
        let job = db.deletion("ses_old".into()).await.unwrap().unwrap();
        assert_eq!(job.anchor_id, "ses_old");
        assert_eq!(job.claimed_ms, None);
        assert_eq!(job.attempts, 0);
        assert_eq!(job.next_attempt_ms, 0);
        let claimed = db.claim_pending_deletions(1, 12).await.unwrap();
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].session_id, "ses_old");
    }

    #[test]
    fn deletion_retry_backoff_is_stable_exponential_and_bounded() {
        let first = deletion_retry_delay_ms("ses_retry", 1);
        assert!((1_000..=1_200).contains(&first));
        assert_eq!(first, deletion_retry_delay_ms("ses_retry", 1));

        let second = deletion_retry_delay_ms("ses_retry", 2);
        assert!((2_000..=2_400).contains(&second));
        let capped = deletion_retry_delay_ms("ses_retry", u32::MAX);
        assert!((240_000..=288_000).contains(&capped));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn file_ledger_uses_rollback_journaling() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "aex-control-journal-{}-{unique}.db",
            std::process::id()
        ));
        let db = Db::open(&path).unwrap();
        let mode: String = db
            .call(|connection| {
                connection.pragma_query_value(None, "journal_mode", |row| row.get(0))
            })
            .await
            .unwrap();
        assert_eq!(mode, "delete");
        drop(db);
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn exact_storage_remainder_survives_process_restart() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "aex-control-storage-fold-{}-{unique}.db",
            std::process::id()
        ));
        {
            let db = Db::open(&path).unwrap();
            db.create_account(
                acct("acc_storage"),
                "storage@example.com".into(),
                "hash".into(),
            )
            .await
            .unwrap();
            let mut fold = FoldState {
                storage_transition_ms: 1_000,
                metered_to_ms: 1_000,
                session_storage_bytes: 3,
                session_state: "open".into(),
                ..FoldState::default()
            };
            crate::rating::accrue_storage(&mut fold, 1_001).unwrap();
            db.insert_session(SessionRow {
                id: "ses_storage".into(),
                account_id: "acc_storage".into(),
                key_id: "key".into(),
                parent_id: None,
                root_id: "ses_storage".into(),
                depth: 0,
                shape: "1gb".into(),
                created_ms: 1_000,
                is_final: false,
                fold,
            })
            .await
            .unwrap();
        }
        {
            let db = Db::open(&path).unwrap();
            let mut row = db.session("ses_storage".into()).await.unwrap().unwrap();
            assert_eq!(row.fold.session_storage_byte_seconds, 0);
            assert_eq!(row.fold.session_storage_byte_millisecond_remainder, 3);
            crate::rating::accrue_storage(&mut row.fold, 1_334).unwrap();
            row.fold.metered_to_ms = 1_334;
            db.apply_sweep(row.id, row.account_id, row.fold, false, 0, 1_334)
                .await
                .unwrap();
        }
        {
            let db = Db::open(&path).unwrap();
            let row = db.session("ses_storage".into()).await.unwrap().unwrap();
            assert_eq!(row.fold.session_storage_byte_seconds, 1);
            assert_eq!(row.fold.session_storage_byte_millisecond_remainder, 2);
            let priced =
                crate::rating::price(&crate::rating::RateCard::default(), "1gb", &row.fold, 1_334)
                    .unwrap();
            assert_eq!(priced.session_storage_byte_milliseconds, 1_002);
        }
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn discovery_watermark_survives_process_restart_and_never_moves_backwards() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "aex-control-discovery-{}-{unique}.db",
            std::process::id()
        ));
        {
            let db = Db::open(&path).unwrap();
            db.create_account(acct("acc_a"), "a@example.com".into(), "h1".into())
                .await
                .unwrap();
            assert_eq!(db.discovery_watermark("acc_a".into()).await.unwrap(), 0);
            db.advance_discovery_watermark("acc_a".into(), 500)
                .await
                .unwrap();
        }
        {
            let db = Db::open(&path).unwrap();
            assert_eq!(db.discovery_watermark("acc_a".into()).await.unwrap(), 500);
            db.advance_discovery_watermark("acc_a".into(), 400)
                .await
                .unwrap();
            assert_eq!(db.discovery_watermark("acc_a".into()).await.unwrap(), 500);
            assert_eq!(db.discovery_accounts().await.unwrap(), vec!["acc_a"]);
        }
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn deletion_phase_survives_process_restart() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "aex-control-deletion-{}-{unique}.db",
            std::process::id()
        ));
        {
            let db = Db::open(&path).unwrap();
            db.create_account(acct("acc_a"), "a@example.com".into(), "h1".into())
                .await
                .unwrap();
            db.insert_session(SessionRow {
                id: "ses_root".into(),
                account_id: "acc_a".into(),
                key_id: "key_1".into(),
                parent_id: None,
                root_id: "ses_root".into(),
                depth: 0,
                shape: "1gb".into(),
                created_ms: 5,
                is_final: false,
                fold: FoldState::default(),
            })
            .await
            .unwrap();
            db.begin_subtree_deletion("acc_a".into(), "ses_root".into(), 10)
                .await
                .unwrap();
            db.set_deletion_phase("ses_root".into(), "settling".into(), 11)
                .await
                .unwrap();
        }
        {
            let db = Db::open(&path).unwrap();
            let job = db.pending_deletions(10).await.unwrap().pop().unwrap();
            assert_eq!(job.session_id, "ses_root");
            assert_eq!(job.phase, "settling");
            assert_eq!(job.accepted_ms, 10);
            assert_eq!(job.updated_ms, 11);
        }
        std::fs::remove_file(path).unwrap();
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
    async fn refund_reservation_is_idempotent_and_failure_restores_credit() {
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
            provider_ref: "fake_top_1".into(),
            checkout_url: None,
            created_ms: 2,
            paid_ms: None,
        })
        .await
        .unwrap();
        db.topup_paid("top_1".into(), 3).await.unwrap();

        let requested = RefundRow {
            id: "rfd_1".into(),
            request_key: "request-1".into(),
            topup_id: "top_1".into(),
            account_id: String::new(),
            amount_cents: 400,
            status: "pending".into(),
            provider_ref: None,
            failure_reason: None,
            created_ms: 4,
            updated_ms: 4,
        };
        let first = db
            .begin_refund(requested.clone(), "fake".into())
            .await
            .unwrap();
        assert_eq!(first.account_id, "acc_a");
        assert_eq!(db.balance("acc_a".into()).await.unwrap(), 6_000_000);

        let mut retry = requested.clone();
        retry.id = "rfd_ignored".into();
        let same = db.begin_refund(retry, "fake".into()).await.unwrap();
        assert_eq!(same.id, first.id, "the request key selects one refund");
        assert_eq!(db.balance("acc_a".into()).await.unwrap(), 6_000_000);

        let mut changed = requested;
        changed.amount_cents = 401;
        let conflict = db.begin_refund(changed, "fake".into()).await.unwrap_err();
        assert!(matches!(conflict, Error::Conflict(_)), "{conflict}");

        db.refund_pending("rfd_1".into(), "fake_refund_1".into(), 5)
            .await
            .unwrap();
        let failed = db
            .refund_failed(
                "rfd_1".into(),
                Some("fake_refund_1".into()),
                "declined".into(),
                6,
            )
            .await
            .unwrap();
        assert_eq!(failed.status, "failed");
        assert_eq!(failed.failure_reason.as_deref(), Some("declined"));
        assert_eq!(
            db.balance("acc_a".into()).await.unwrap(),
            10_000_000,
            "a failed provider attempt releases the reservation"
        );

        let succeeded = db
            .begin_refund(
                RefundRow {
                    id: "rfd_2".into(),
                    request_key: "request-2".into(),
                    topup_id: "top_1".into(),
                    account_id: String::new(),
                    amount_cents: 1000,
                    status: "pending".into(),
                    provider_ref: None,
                    failure_reason: None,
                    created_ms: 7,
                    updated_ms: 7,
                },
                "fake".into(),
            )
            .await
            .unwrap();
        db.refund_succeeded(succeeded.id, "fake_refund_2".into(), 8)
            .await
            .unwrap();
        assert_eq!(db.balance("acc_a".into()).await.unwrap(), 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn refund_cannot_exceed_the_topup_or_unused_account_balance() {
        let db = Db::open_memory().unwrap();
        db.create_account(acct("acc_a"), "a@example.com".into(), "h1".into())
            .await
            .unwrap();
        for id in ["top_1", "top_2"] {
            db.create_topup(TopupRow {
                id: id.into(),
                account_id: "acc_a".into(),
                amount_cents: 1000,
                status: "pending".into(),
                provider: "fake".into(),
                provider_ref: format!("fake_{id}"),
                checkout_url: None,
                created_ms: 2,
                paid_ms: None,
            })
            .await
            .unwrap();
            db.topup_paid(id.into(), 3).await.unwrap();
        }
        db.call(|connection| {
            connection.execute(
                "INSERT INTO ledger (ref, account_id, microusd, updated_ms)
                 VALUES ('usage:test', 'acc_a', -15000000, 4)",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        let refund = |id: &str, topup_id: &str, cents| RefundRow {
            id: id.into(),
            request_key: format!("request-{id}"),
            topup_id: topup_id.into(),
            account_id: String::new(),
            amount_cents: cents,
            status: "pending".into(),
            provider_ref: None,
            failure_reason: None,
            created_ms: 5,
            updated_ms: 5,
        };
        let unavailable = db
            .begin_refund(refund("rfd_1", "top_1", 600), "fake".into())
            .await
            .unwrap_err();
        assert!(matches!(unavailable, Error::InsufficientBalance(_)));
        let too_much = db
            .begin_refund(refund("rfd_2", "top_1", 1001), "fake".into())
            .await
            .unwrap_err();
        assert!(matches!(too_much, Error::Conflict(_)));
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
            parent_id: None,
            root_id: "ses_1".into(),
            depth: 0,
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
        // Absolute replacement may move down when a delayed durable transition shortens an
        // earlier open-interval estimate, then up again when a later transition reveals usage.
        row.fold.folded_seq = 30;
        row.fold.metered_to_ms = 300;
        assert!(
            db.apply_sweep(
                "ses_1".into(),
                "acc_a".into(),
                row.fold.clone(),
                false,
                400,
                300
            )
            .await
            .unwrap()
        );
        assert_eq!(db.balance("acc_a".into()).await.unwrap(), -400);
        row.fold.folded_seq = 40;
        row.fold.metered_to_ms = 400;
        assert!(
            db.apply_sweep(
                "ses_1".into(),
                "acc_a".into(),
                row.fold.clone(),
                false,
                1_200,
                400
            )
            .await
            .unwrap()
        );
        assert_eq!(db.balance("acc_a".into()).await.unwrap(), -1_200);
        // A stale sweep (lower fence) writes nothing.
        row.fold.folded_seq = 35;
        row.fold.metered_to_ms = 350;
        assert!(
            !db.apply_sweep(
                "ses_1".into(),
                "acc_a".into(),
                row.fold.clone(),
                false,
                9999,
                350
            )
            .await
            .unwrap()
        );
        assert_eq!(db.balance("acc_a".into()).await.unwrap(), -1_200);
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
        db.key_auth("sh1".into(), 10).await.unwrap().unwrap();
        assert_eq!(
            db.list_keys("acc_a".into()).await.unwrap()[0].last_used_ms,
            Some(9),
            "ordinary requests do not force one EFS journal write apiece"
        );
        db.key_auth("sh1".into(), 60_009).await.unwrap().unwrap();
        assert_eq!(
            db.list_keys("acc_a".into()).await.unwrap()[0].last_used_ms,
            Some(60_009),
            "the audit timestamp still advances at its bounded cadence"
        );
        assert!(
            db.revoke_key("acc_a".into(), "key_1".into(), 60_010)
                .await
                .unwrap()
        );
        assert!(db.key_auth("sh1".into(), 60_011).await.unwrap().is_none());
        assert!(
            !db.revoke_key("acc_a".into(), "key_1".into(), 60_012)
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
    async fn waitlist_invitation_is_canonical_and_one_time() {
        let db = Db::open_memory().unwrap();
        let first = db
            .join_waitlist("Alpha@Example.com".into(), 1)
            .await
            .unwrap();
        assert_eq!(first.status, "waiting");
        let repeated = db
            .join_waitlist("alpha@example.com".into(), 2)
            .await
            .unwrap();
        assert_eq!(repeated.created_ms, 1, "a repeat does not reset queue age");

        db.invite_waitlist("alpha@example.com".into(), "invite-hash".into(), 3)
            .await
            .unwrap()
            .unwrap();
        let wrong = db
            .create_invited_account(
                acct("acc_wrong"),
                "alpha@example.com".into(),
                "account-hash-wrong".into(),
                "wrong-invite".into(),
                4,
            )
            .await
            .unwrap_err();
        assert!(matches!(wrong, Error::Forbidden(_)));

        db.create_invited_account(
            acct("acc_beta"),
            "alpha@example.com".into(),
            "account-hash".into(),
            "invite-hash".into(),
            5,
        )
        .await
        .unwrap();
        let rows = db.list_waitlist().await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, "joined");
        assert_eq!(rows[0].joined_ms, Some(5));
        assert!(
            db.invite_waitlist("alpha@example.com".into(), "again".into(), 6)
                .await
                .unwrap()
                .is_none(),
            "a joined address cannot be re-invited"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn account_create_limits_count_resource_bearing_roots_only() {
        let db = Db::open_memory().unwrap();
        db.create_account(acct("acc_a"), "a@example.com".into(), "h1".into())
            .await
            .unwrap();
        for (id, state, is_final) in [
            ("ses_a", "open", false),
            ("ses_b", "open", false),
            ("ses_c", "ending", false),
            ("ses_d", "open", true),
            ("ses_e", "ended", false),
            ("ses_f", "failed", false),
            ("ses_g", "deleting", false),
        ] {
            let mut row = SessionRow {
                id: id.into(),
                account_id: "acc_a".into(),
                key_id: "key_1".into(),
                parent_id: None,
                root_id: id.into(),
                depth: 0,
                shape: "1gb".into(),
                created_ms: 5,
                is_final: false,
                fold: FoldState::default(),
            };
            db.insert_session(row.clone()).await.unwrap();
            row.fold.session_state = state.into();
            db.apply_sweep(id.into(), "acc_a".into(), row.fold, is_final, 0, 6)
                .await
                .unwrap();
        }
        db.insert_session(SessionRow {
            id: "ses_child".into(),
            account_id: "acc_a".into(),
            key_id: "key_1".into(),
            parent_id: Some("ses_a".into()),
            root_id: "ses_a".into(),
            depth: 1,
            shape: "1gb".into(),
            created_ms: 5,
            is_final: false,
            fold: FoldState {
                session_state: "open".into(),
                ..Default::default()
            },
        })
        .await
        .unwrap();
        assert_eq!(db.live_root_count("acc_a".into()).await.unwrap(), 5);
        assert_eq!(db.root_creates_since("acc_a".into(), 0).await.unwrap(), 7);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn accepted_root_deletion_keeps_capacity_until_physical_completion() {
        let db = Db::open_memory().unwrap();
        db.create_account(acct("acc_a"), "a@example.com".into(), "h1".into())
            .await
            .unwrap();
        let row = SessionRow {
            id: "ses_root".into(),
            account_id: "acc_a".into(),
            key_id: "key_1".into(),
            parent_id: None,
            root_id: "ses_root".into(),
            depth: 0,
            shape: "1gb".into(),
            created_ms: 5,
            is_final: false,
            fold: FoldState {
                session_state: "open".into(),
                ..Default::default()
            },
        };
        db.insert_session(row.clone()).await.unwrap();

        db.begin_subtree_deletion("acc_a".into(), "ses_root".into(), 10)
            .await
            .unwrap();
        assert_eq!(db.live_root_count("acc_a".into()).await.unwrap(), 1);
        for (phase, at) in [("settling", 11), ("purging", 12), ("awaiting_purge", 13)] {
            db.set_deletion_phase("ses_root".into(), phase.into(), at)
                .await
                .unwrap();
            assert_eq!(
                db.live_root_count("acc_a".into()).await.unwrap(),
                1,
                "a locally advanced deletion phase is not a sandbox-release proof"
            );
        }

        let mut deleted = row.fold;
        deleted.session_state = "deleted".into();
        db.apply_sweep("ses_root".into(), "acc_a".into(), deleted, true, 0, 14)
            .await
            .unwrap();
        db.finish_subtree_deletion("acc_a".into(), "ses_root".into())
            .await
            .unwrap();
        assert_eq!(db.live_root_count("acc_a".into()).await.unwrap(), 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn subtree_deletion_tracks_one_anchor_and_selects_only_descendants() {
        let db = Db::open_memory().unwrap();
        db.create_account(acct("acc_a"), "a@example.com".into(), "h1".into())
            .await
            .unwrap();
        for (id, parent_id, root_id) in [
            ("ses_root", None, "ses_root"),
            ("ses_child", Some("ses_root"), "ses_root"),
            ("ses_grandchild", Some("ses_child"), "ses_root"),
            ("ses_sibling_root", None, "ses_sibling_root"),
        ] {
            db.insert_session(SessionRow {
                id: id.into(),
                account_id: "acc_a".into(),
                key_id: "key_1".into(),
                parent_id: parent_id.map(str::to_owned),
                root_id: root_id.into(),
                depth: match id {
                    "ses_grandchild" => 2,
                    "ses_child" => 1,
                    _ => 0,
                },
                shape: "1gb".into(),
                created_ms: 5,
                is_final: false,
                fold: FoldState {
                    session_state: "open".into(),
                    ..Default::default()
                },
            })
            .await
            .unwrap();
        }
        db.begin_subtree_deletion("acc_a".into(), "ses_child".into(), 10)
            .await
            .unwrap();
        assert_eq!(
            db.deletion_roots("acc_a".into()).await.unwrap(),
            vec!["ses_child"]
        );
        let accepted = db.deletion("ses_child".into()).await.unwrap().unwrap();
        assert_eq!(accepted.phase, "ending");
        assert_eq!(accepted.accepted_ms, 10);
        assert_eq!(db.pending_deletions(10).await.unwrap().len(), 1);
        db.set_deletion_phase("ses_child".into(), "settling".into(), 11)
            .await
            .unwrap();
        db.record_deletion_error("ses_child".into(), "retry me".into(), 12)
            .await
            .unwrap();
        let retry = db.deletion("ses_child".into()).await.unwrap().unwrap();
        assert_eq!(retry.phase, "settling");
        assert_eq!(retry.last_error.as_deref(), Some("retry me"));
        assert_eq!(retry.attempts, 1);
        assert!(retry.next_attempt_ms >= 1_012);
        assert!(retry.next_attempt_ms <= 1_212);
        assert!(
            db.claim_pending_deletions(1, retry.next_attempt_ms - 1)
                .await
                .unwrap()
                .is_empty()
        );
        let due = db
            .claim_pending_deletions(1, retry.next_attempt_ms)
            .await
            .unwrap();
        assert_eq!(due.len(), 1);
        db.release_deletion_claim("ses_child".into()).await.unwrap();
        db.set_deletion_phase("ses_child".into(), "settling".into(), retry.next_attempt_ms)
            .await
            .unwrap();
        let reset = db.deletion("ses_child".into()).await.unwrap().unwrap();
        assert_eq!(reset.attempts, 0);
        assert_eq!(reset.next_attempt_ms, retry.next_attempt_ms);
        let selected = db
            .subtree_sessions("acc_a".into(), "ses_child".into())
            .await
            .unwrap();
        assert_eq!(selected.len(), 2);
        assert!(
            selected
                .iter()
                .all(|row| row.fold.session_state == "deleting")
        );
        assert_eq!(
            db.session("ses_root".into())
                .await
                .unwrap()
                .unwrap()
                .fold
                .session_state,
            "open"
        );
        db.record_hosted_tool_response(
            "ses_child".into(),
            "call_child".into(),
            "hash_child".into(),
            r#"{"content":"sensitive child result"}"#.into(),
            12,
        )
        .await
        .unwrap();
        db.record_hosted_tool_response(
            "ses_root".into(),
            "call_root".into(),
            "hash_root".into(),
            r#"{"content":"retained sibling payload"}"#.into(),
            12,
        )
        .await
        .unwrap();
        db.call(|connection| {
            connection.execute(
                "INSERT INTO output_requests
                   (id, session_id, schema_hash, schema_json, max_attempts, attempts, status,
                    request_hash, created_ms)
                 VALUES ('out_child', 'ses_child', 'schema', '{}', 2, 1, 'completed',
                         'request', 12)",
                [],
            )?;
            connection.execute(
                "INSERT INTO external_tool_calls
                   (session_id, call_id, output_id, response_json, created_ms)
                 VALUES ('ses_child', 'output_call', 'out_child',
                         '{\"content\":\"sensitive structured result\"}', 12)",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        db.finish_subtree_deletion("acc_a".into(), "ses_child".into())
            .await
            .unwrap();
        assert!(db.deletion_roots("acc_a".into()).await.unwrap().is_empty());
        let completed = db.deletion("ses_child".into()).await.unwrap().unwrap();
        assert_eq!(completed.phase, "succeeded");
        assert!(completed.completed_ms.is_some());
        assert!(completed.last_error.is_none());
        assert_eq!(completed.attempts, 0);
        assert!(
            db.hosted_tool_response("ses_child".into(), "call_child".into(), "hash_child".into())
                .await
                .unwrap()
                .is_none(),
            "deleted subtree payloads must not survive the completion boundary"
        );
        assert!(
            db.hosted_tool_response("ses_root".into(), "call_root".into(), "hash_root".into())
                .await
                .unwrap()
                .is_some(),
            "deleting one subtree must not purge its ancestor's payload"
        );
        let retained_sensitive_rows = db
            .call(|connection| {
                connection.query_row(
                    "SELECT
                       (SELECT COUNT(*) FROM output_requests WHERE session_id = 'ses_child') +
                       (SELECT COUNT(*) FROM external_tool_calls WHERE session_id = 'ses_child')",
                    [],
                    |record| record.get::<_, i64>(0),
                )
            })
            .await
            .unwrap();
        assert_eq!(retained_sensitive_rows, 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn root_first_deletion_aliases_descendants_and_recovers_claims() {
        let db = deletion_tree().await;
        db.begin_subtree_deletion("acc_delete".into(), "ses_root".into(), 10)
            .await
            .unwrap();
        db.begin_subtree_deletion("acc_delete".into(), "ses_root".into(), 11)
            .await
            .unwrap();
        db.begin_subtree_deletion("acc_delete".into(), "ses_child_a".into(), 12)
            .await
            .unwrap();
        db.begin_subtree_deletion("acc_delete".into(), "ses_grandchild".into(), 13)
            .await
            .unwrap();

        assert_eq!(db.pending_deletions(10).await.unwrap().len(), 3);
        assert_eq!(
            db.deletion("ses_child_a".into())
                .await
                .unwrap()
                .unwrap()
                .anchor_id,
            "ses_root"
        );
        assert_eq!(
            db.deletion("ses_grandchild".into())
                .await
                .unwrap()
                .unwrap()
                .anchor_id,
            "ses_root"
        );
        let first = db.claim_pending_deletions(8, 20).await.unwrap();
        assert_eq!(
            first
                .iter()
                .map(|job| job.session_id.as_str())
                .collect::<Vec<_>>(),
            ["ses_root"]
        );

        // A process restart reclaims only the executable root, never either durable alias.
        db.clear_deletion_claims().await.unwrap();
        let recovered = db.claim_pending_deletions(8, 21).await.unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].session_id, "ses_root");

        db.finish_subtree_deletion("acc_delete".into(), "ses_root".into())
            .await
            .unwrap();
        for id in ["ses_root", "ses_child_a", "ses_grandchild"] {
            assert_eq!(
                db.deletion(id.into()).await.unwrap().unwrap().phase,
                "succeeded"
            );
        }
        assert!(db.claim_pending_deletions(8, 22).await.unwrap().is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn root_first_deletion_hydrates_a_hidden_deep_child_before_aliasing_and_restart() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "aex-control-hidden-child-{}-{unique}.db",
            std::process::id()
        ));
        let session = |id: &str, parent_id: Option<&str>, depth: i64| SessionRow {
            id: id.into(),
            account_id: "acc_hidden".into(),
            key_id: "key_hidden".into(),
            parent_id: parent_id.map(str::to_owned),
            root_id: "ses_root".into(),
            depth,
            shape: "1gb".into(),
            created_ms: 5 + depth,
            is_final: false,
            fold: FoldState {
                session_state: "open".into(),
                ..Default::default()
            },
        };

        {
            let db = Db::open(&path).unwrap();
            db.create_account(
                acct("acc_hidden"),
                "hidden@example.com".into(),
                "hidden-token".into(),
            )
            .await
            .unwrap();
            db.insert_session(session("ses_root", None, 0))
                .await
                .unwrap();
            db.begin_subtree_deletion("acc_hidden".into(), "ses_root".into(), 10)
                .await
                .unwrap();
            assert_eq!(
                db.claim_pending_deletions(8, 11).await.unwrap()[0].session_id,
                "ses_root"
            );

            // Neither descendant existed in the local discovery index when the root was accepted.
            // A strong Brain parent walk supplies the complete chain in selected-to-root order.
            db.begin_subtree_deletion_with_chain(
                "acc_hidden".into(),
                "ses_grandchild".into(),
                vec![
                    session("ses_grandchild", Some("ses_child"), 2),
                    session("ses_child", Some("ses_root"), 1),
                    session("ses_root", None, 0),
                ],
                12,
            )
            .await
            .unwrap();
            let alias = db.deletion("ses_grandchild".into()).await.unwrap().unwrap();
            assert_eq!(alias.anchor_id, "ses_root");
            assert!(db.session("ses_child".into()).await.unwrap().is_some());
        }

        {
            let db = Db::open(&path).unwrap();
            db.clear_deletion_claims().await.unwrap();
            let recovered = db.claim_pending_deletions(8, 20).await.unwrap();
            assert_eq!(recovered.len(), 1);
            assert_eq!(recovered[0].session_id, "ses_root");
            assert_eq!(
                db.deletion("ses_grandchild".into())
                    .await
                    .unwrap()
                    .unwrap()
                    .anchor_id,
                "ses_root"
            );
        }
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn child_admitted_after_ancestor_completion_is_a_succeeded_alias() {
        let db = deletion_tree().await;
        db.begin_subtree_deletion("acc_delete".into(), "ses_root".into(), 10)
            .await
            .unwrap();
        db.finish_subtree_deletion("acc_delete".into(), "ses_root".into())
            .await
            .unwrap();
        let hidden = SessionRow {
            id: "ses_late_hidden".into(),
            account_id: "acc_delete".into(),
            key_id: "key_delete".into(),
            parent_id: Some("ses_root".into()),
            root_id: "ses_root".into(),
            depth: 1,
            shape: "1gb".into(),
            created_ms: 6,
            is_final: true,
            fold: FoldState {
                session_state: "deleted".into(),
                ..Default::default()
            },
        };
        let root = db.session("ses_root".into()).await.unwrap().unwrap();
        db.begin_subtree_deletion_with_chain(
            "acc_delete".into(),
            hidden.id.clone(),
            vec![hidden.clone(), root],
            20,
        )
        .await
        .unwrap();
        let alias = db.deletion(hidden.id).await.unwrap().unwrap();
        assert_eq!(alias.anchor_id, "ses_root");
        assert_eq!(alias.phase, "succeeded");
        assert!(alias.completed_ms.is_some());
        assert!(db.claim_pending_deletions(8, 21).await.unwrap().is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn child_first_deletion_blocks_ancestor_but_allows_siblings() {
        let db = deletion_tree().await;
        db.begin_subtree_deletion("acc_delete".into(), "ses_child_a".into(), 10)
            .await
            .unwrap();
        let claimed_child = db.claim_pending_deletions(8, 11).await.unwrap();
        assert_eq!(claimed_child.len(), 1);
        assert_eq!(claimed_child[0].session_id, "ses_child_a");

        // Accepting the ancestor while its child is already claimed creates a durable dependency.
        db.begin_subtree_deletion("acc_delete".into(), "ses_root".into(), 12)
            .await
            .unwrap();
        db.begin_subtree_deletion("acc_delete".into(), "ses_root".into(), 13)
            .await
            .unwrap();
        assert!(db.claim_pending_deletions(8, 14).await.unwrap().is_empty());

        // Simulate a crash between claim and execution. The same child resumes first; the parent
        // cannot be claimed until its durable prerequisite succeeds.
        db.clear_deletion_claims().await.unwrap();
        let recovered_child = db.claim_pending_deletions(8, 15).await.unwrap();
        assert_eq!(recovered_child.len(), 1);
        assert_eq!(recovered_child[0].session_id, "ses_child_a");
        db.finish_subtree_deletion("acc_delete".into(), "ses_child_a".into())
            .await
            .unwrap();
        let root = db.claim_pending_deletions(8, 16).await.unwrap();
        assert_eq!(root.len(), 1);
        assert_eq!(root[0].session_id, "ses_root");
        db.finish_subtree_deletion("acc_delete".into(), "ses_root".into())
            .await
            .unwrap();
        assert!(db.pending_deletions(8).await.unwrap().is_empty());

        let siblings = deletion_tree().await;
        siblings
            .begin_subtree_deletion("acc_delete".into(), "ses_child_a".into(), 20)
            .await
            .unwrap();
        siblings
            .begin_subtree_deletion("acc_delete".into(), "ses_child_b".into(), 21)
            .await
            .unwrap();
        let parallel = siblings.claim_pending_deletions(8, 22).await.unwrap();
        assert_eq!(parallel.len(), 2);
        assert_eq!(
            parallel
                .iter()
                .map(|job| job.session_id.as_str())
                .collect::<std::collections::HashSet<_>>(),
            std::collections::HashSet::from(["ses_child_a", "ses_child_b"])
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn create_idempotency_mapping_and_live_increment_identity_are_exactly_once() {
        let db = Db::open_memory().unwrap();
        db.create_account(acct("acc_a"), "a@example.com".into(), "h1".into())
            .await
            .unwrap();
        let row = SessionRow {
            id: "ses_created".into(),
            account_id: "acc_a".into(),
            key_id: "key_1".into(),
            parent_id: None,
            root_id: "ses_created".into(),
            depth: 0,
            shape: "1gb".into(),
            created_ms: 5,
            is_final: false,
            fold: FoldState::default(),
        };
        assert!(
            db.record_created_session(
                row.clone(),
                Some(("key-hash".into(), "request-hash".into(), 5))
            )
            .await
            .unwrap()
        );
        assert_eq!(
            db.session_create_request("acc_a".into(), "key-hash".into())
                .await
                .unwrap(),
            Some(SessionCreateRow {
                request_hash: "request-hash".into(),
                session_id: "ses_created".into(),
            })
        );
        assert!(
            !db.record_created_session(
                row.clone(),
                Some(("key-hash".into(), "request-hash".into(), 6))
            )
            .await
            .unwrap(),
            "a concurrent or later replay must not increment the live cache twice"
        );
        assert!(
            db.record_created_session(
                row,
                Some(("key-hash".into(), "different-request".into(), 7))
            )
            .await
            .is_err()
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn retained_root_quota_is_atomic_and_releases_only_after_physical_delete() {
        let db = Db::open_memory().unwrap();
        db.create_account(acct("acc_a"), "a@example.com".into(), "h1".into())
            .await
            .unwrap();
        let root = SessionRow {
            id: "ses_ended".into(),
            account_id: "acc_a".into(),
            key_id: "key_1".into(),
            parent_id: None,
            root_id: "ses_ended".into(),
            depth: 0,
            shape: "1gb".into(),
            created_ms: 1,
            is_final: false,
            fold: FoldState {
                session_state: "ended".into(),
                ..FoldState::default()
            },
        };
        db.insert_session(root.clone()).await.unwrap();
        assert_eq!(db.retained_root_slots("acc_a".into()).await.unwrap(), 1);
        assert_eq!(
            db.ensure_session_create_intent(
                "acc_a".into(),
                "blocked".into(),
                "request".into(),
                2,
                1,
            )
            .await
            .unwrap(),
            SessionCreateIntent::RetainedRootLimit,
            "logical end must not release retained journal capacity",
        );

        let mut deleted = root.fold;
        deleted.session_state = "deleted".into();
        deleted.metered_to_ms = 3;
        assert!(
            db.apply_sweep("ses_ended".into(), "acc_a".into(), deleted, true, 0, 3)
                .await
                .unwrap()
        );
        assert_eq!(db.retained_root_slots("acc_a".into()).await.unwrap(), 0);
        assert_eq!(
            db.ensure_session_create_intent(
                "acc_a".into(),
                "replacement".into(),
                "request".into(),
                4,
                1,
            )
            .await
            .unwrap(),
            SessionCreateIntent::Created,
        );
        assert_eq!(db.retained_root_slots("acc_a".into()).await.unwrap(), 1);
        assert_eq!(
            db.ensure_session_create_intent(
                "acc_a".into(),
                "replacement".into(),
                "request".into(),
                5,
                1,
            )
            .await
            .unwrap(),
            SessionCreateIntent::Existing,
            "same-key recovery keeps access to its already-reserved identity at the cap",
        );
        assert_eq!(
            db.ensure_session_create_intent(
                "acc_a".into(),
                "second".into(),
                "request".into(),
                5,
                1,
            )
            .await
            .unwrap(),
            SessionCreateIntent::RetainedRootLimit,
        );

        let replacement = SessionRow {
            id: "ses_replacement".into(),
            account_id: "acc_a".into(),
            key_id: "key_1".into(),
            parent_id: None,
            root_id: "ses_replacement".into(),
            depth: 0,
            shape: "1gb".into(),
            created_ms: 6,
            is_final: false,
            fold: FoldState::default(),
        };
        db.upsert_discovered_sessions(vec![replacement.clone()])
            .await
            .unwrap();
        assert!(
            db.session_create_intent("acc_a".into(), "replacement".into())
                .await
                .unwrap()
                .unwrap()
                .covered
        );
        assert_eq!(
            db.retained_root_slots("acc_a".into()).await.unwrap(),
            1,
            "covering an ambiguous identity with its discovered root must not double count",
        );
        let mut deleted = replacement.fold;
        deleted.session_state = "deleted".into();
        deleted.metered_to_ms = 7;
        assert!(
            db.apply_sweep(
                "ses_replacement".into(),
                "acc_a".into(),
                deleted,
                true,
                0,
                7,
            )
            .await
            .unwrap()
        );
        db.finish_subtree_deletion("acc_a".into(), "ses_replacement".into())
            .await
            .unwrap();
        assert!(
            db.session_create_intent("acc_a".into(), "replacement".into())
                .await
                .unwrap()
                .is_none(),
            "physical deletion removes the covered ambiguous reservation",
        );
        assert_eq!(db.retained_root_slots("acc_a".into()).await.unwrap(), 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn racing_durable_create_intents_cannot_cross_the_retained_root_cap() {
        let db = Db::open_memory().unwrap();
        db.create_account(acct("acc_a"), "a@example.com".into(), "h1".into())
            .await
            .unwrap();
        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(100));
        let mut tasks = Vec::new();
        for index in 0..100 {
            let db = db.clone();
            let barrier = barrier.clone();
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                db.ensure_session_create_intent(
                    "acc_a".into(),
                    format!("key-{index}"),
                    format!("request-{index}"),
                    1,
                    10,
                )
                .await
                .unwrap()
            }));
        }
        let mut created = 0;
        let mut limited = 0;
        for task in tasks {
            match task.await.unwrap() {
                SessionCreateIntent::Created => created += 1,
                SessionCreateIntent::RetainedRootLimit => limited += 1,
                outcome => panic!("unexpected durable create outcome: {outcome:?}"),
            }
        }
        assert_eq!((created, limited), (10, 90));
        assert_eq!(db.retained_root_slots("acc_a".into()).await.unwrap(), 10);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn uncertain_create_slots_survive_restart_and_strong_discovery_covers_once() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "aex-control-create-intent-{}-{unique}.db",
            std::process::id()
        ));
        {
            let db = Db::open(&path).unwrap();
            db.create_account(acct("acc_a"), "a@example.com".into(), "h1".into())
                .await
                .unwrap();
            assert_eq!(
                db.ensure_session_create_intent(
                    "acc_a".into(),
                    "key-a".into(),
                    "request-a".into(),
                    5,
                    100,
                )
                .await
                .unwrap(),
                SessionCreateIntent::Created
            );
            db.mark_session_create_uncertain("acc_a".into(), "key-a".into(), "request-a".into(), 6)
                .await
                .unwrap();
        }
        {
            let db = Db::open(&path).unwrap();
            assert_eq!(
                db.uncovered_session_create_intents("acc_a".into())
                    .await
                    .unwrap(),
                1,
                "a restart must not silently free an ambiguous root slot"
            );
            assert_eq!(
                db.ensure_session_create_intent(
                    "acc_a".into(),
                    "key-a".into(),
                    "request-a".into(),
                    7,
                    100,
                )
                .await
                .unwrap(),
                SessionCreateIntent::Existing
            );
            assert_eq!(
                db.ensure_session_create_intent(
                    "acc_a".into(),
                    "key-a".into(),
                    "different".into(),
                    7,
                    100,
                )
                .await
                .unwrap(),
                SessionCreateIntent::Conflict
            );
            assert_eq!(
                db.ensure_session_create_intent(
                    "acc_a".into(),
                    "key-b".into(),
                    "request-b".into(),
                    8,
                    100,
                )
                .await
                .unwrap(),
                SessionCreateIntent::Created
            );
            let row = SessionRow {
                id: "ses_discovered".into(),
                account_id: "acc_a".into(),
                key_id: "key_1".into(),
                parent_id: None,
                root_id: "ses_discovered".into(),
                depth: 0,
                shape: "1gb".into(),
                created_ms: 9,
                is_final: false,
                fold: FoldState {
                    session_state: "open".into(),
                    ..Default::default()
                },
            };
            db.upsert_discovered_sessions(vec![row.clone()])
                .await
                .unwrap();
            assert_eq!(
                db.uncovered_session_create_intents("acc_a".into())
                    .await
                    .unwrap(),
                1,
                "one observed root covers exactly one possible commit"
            );
            db.record_created_session(row, Some(("key-a".into(), "request-a".into(), 10)))
                .await
                .unwrap();
            db.abandon_session_create_intent("acc_a".into(), "key-b".into(), "request-b".into())
                .await
                .unwrap();
            assert_eq!(
                db.uncovered_session_create_intents("acc_a".into())
                    .await
                    .unwrap(),
                0
            );
        }
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hosted_tool_results_replay_exactly_and_reject_identity_reuse() {
        let db = Db::open_memory().unwrap();
        assert!(
            db.hosted_tool_response("ses_a".into(), "call_a".into(), "hash_a".into())
                .await
                .unwrap()
                .is_none()
        );
        let first = db
            .record_hosted_tool_response(
                "ses_a".into(),
                "call_a".into(),
                "hash_a".into(),
                r#"{"result":1}"#.into(),
                1,
            )
            .await
            .unwrap();
        let replay = db
            .record_hosted_tool_response(
                "ses_a".into(),
                "call_a".into(),
                "hash_a".into(),
                r#"{"result":2}"#.into(),
                2,
            )
            .await
            .unwrap();
        assert_eq!(first, replay);
        assert!(matches!(
            db.hosted_tool_response("ses_a".into(), "call_a".into(), "different".into())
                .await,
            Err(Error::Conflict(_))
        ));
    }
}
