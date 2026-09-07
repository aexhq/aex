use crate::{
    config::Limits,
    error::{Error, Result},
    identity::{Principal, digest, matches, random},
};
use sqlx::{
    Row, SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};
use std::{
    path::Path,
    str::FromStr,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Clone)]
pub struct Store(pub SqlitePool);
pub enum Claim {
    New(String),
    Complete(serde_json::Value),
}
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_secs() as i64
}

impl Store {
    pub async fn open(path: &Path) -> anyhow::Result<Self> {
        let options = SqliteConnectOptions::from_str("sqlite:")?
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Full)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(5))
            .connect_with(options)
            .await?;
        sqlx::migrate!().run(&pool).await?;
        Ok(Self(pool))
    }
    pub async fn principal(&self, token: &str) -> Result<Principal> {
        let row = sqlx::query("SELECT k.id,k.account FROM api_keys k JOIN accounts a ON a.id=k.account WHERE k.verifier=? AND k.active=1 AND a.active=1")
            .bind(digest(token.as_bytes())).fetch_optional(&self.0).await?.ok_or_else(Error::denied)?;
        Ok(Principal {
            account: row.get("account"),
            key: row.get("id"),
        })
    }
    pub async fn active(&self, p: &Principal) -> Result<()> {
        let active: i64 = sqlx::query_scalar("SELECT count(*) FROM api_keys k JOIN accounts a ON a.id=k.account WHERE k.id=? AND k.account=? AND k.active=1 AND a.active=1")
            .bind(&p.key).bind(&p.account).fetch_one(&self.0).await?;
        if active == 1 {
            Ok(())
        } else {
            Err(Error::denied())
        }
    }
    pub async fn host(&self, id: &str, token: &str) -> Result<Principal> {
        let row = sqlx::query("SELECT account,issuing_key,verifier FROM hosts WHERE id=?")
            .bind(id)
            .fetch_optional(&self.0)
            .await?
            .ok_or_else(Error::denied)?;
        if !matches(token, row.get("verifier")) {
            return Err(Error::denied());
        }
        let p = Principal {
            account: row.get("account"),
            key: row.get("issuing_key"),
        };
        self.active(&p).await?;
        Ok(p)
    }
    pub async fn own_host(&self, p: &Principal, id: &str) -> Result<()> {
        let key: Option<String> =
            sqlx::query_scalar("SELECT issuing_key FROM hosts WHERE id=? AND account=?")
                .bind(id)
                .bind(&p.account)
                .fetch_optional(&self.0)
                .await?;
        self.active(&Principal {
            account: p.account.clone(),
            key: key.ok_or_else(Error::missing)?,
        })
        .await
    }
    pub async fn owned(&self, p: &Principal, id: &str, deleting: bool) -> Result<()> {
        let state: Option<String> =
            sqlx::query_scalar("SELECT state FROM sessions WHERE id=? AND account=?")
                .bind(id)
                .bind(&p.account)
                .fetch_optional(&self.0)
                .await?;
        match state.as_deref() {
            Some("owned") => Ok(()),
            Some("deleting" | "deleted") if deleting => Ok(()),
            _ => Err(Error::missing()),
        }
    }
    pub async fn session_ids(&self, account: &str) -> Result<Vec<String>> {
        Ok(sqlx::query_scalar(
            "SELECT id FROM sessions WHERE account=? AND state='owned' ORDER BY created,id",
        )
        .bind(account)
        .fetch_all(&self.0)
        .await?)
    }
    pub async fn claim(
        &self,
        p: &Principal,
        operation: &str,
        key: &str,
        fingerprint: &str,
        limits: &Limits,
    ) -> Result<Claim> {
        let mut tx = self.0.begin().await?;
        if let Some(row) = sqlx::query("SELECT fingerprint,state,result FROM claims WHERE account=? AND operation=? AND client_key=?")
            .bind(&p.account).bind(operation).bind(key).fetch_optional(&mut *tx).await? {
            if row.get::<String,_>("fingerprint") != fingerprint { return Err(Error::conflict("operation key reused with different request")); }
            return if row.get::<String,_>("state") == "complete" {
                Ok(Claim::Complete(serde_json::from_str(row.get("result"))?))
            } else { Err(Error::ambiguous()) };
        }
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM claims WHERE account=?")
            .bind(&p.account)
            .fetch_one(&mut *tx)
            .await?;
        if count >= i64::from(limits.claims_per_account) {
            return Err(Error::capacity());
        }
        if operation == "create" {
            let count: i64 = sqlx::query_scalar("SELECT (SELECT count(*) FROM sessions WHERE account=? AND state!='deleted') + (SELECT count(*) FROM claims WHERE account=? AND operation='create' AND state='pending')")
                .bind(&p.account).bind(&p.account).fetch_one(&mut *tx).await?;
            if count >= i64::from(limits.sessions_per_account) {
                return Err(Error::capacity());
            }
            let bytes: i64 = sqlx::query_scalar("SELECT coalesce(sum(retained_bytes),0) FROM sessions WHERE account=? AND state!='deleted'").bind(&p.account).fetch_one(&mut *tx).await?;
            if (bytes as u64)
                .saturating_add((count as u64 + 1).saturating_mul(limits.turn_reserve_bytes))
                > limits.retained_bytes_per_account
            {
                return Err(Error::capacity());
            }
        }
        let upstream = random("op");
        sqlx::query("INSERT INTO claims VALUES (?,?,?,?,?,'pending',NULL,?)")
            .bind(&p.account)
            .bind(operation)
            .bind(key)
            .bind(fingerprint)
            .bind(&upstream)
            .bind(now())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(Claim::New(upstream))
    }
    pub async fn complete_create(
        &self,
        p: &Principal,
        upstream: &str,
        summary: &brain_protocol::SessionSummary,
        initial_bytes: u64,
    ) -> Result<()> {
        let mut tx = self.0.begin().await?;
        sqlx::query(
            "INSERT INTO sessions(id,account,created,retained_bytes,changed_at) VALUES (?,?,?,?,?)",
        )
        .bind(summary.session_id.as_str())
        .bind(&p.account)
        .bind(now())
        .bind(initial_bytes as i64)
        .bind(now())
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE claims SET state='complete',result=? WHERE upstream_key=? AND state='pending'",
        )
        .bind(serde_json::to_string(summary)?)
        .bind(upstream)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn register_host(
        &self,
        p: &Principal,
        host: &brain_protocol::HostRegistration,
    ) -> Result<()> {
        sqlx::query("INSERT INTO hosts VALUES (?,?,?,?,?)")
            .bind(host.host_id.as_str())
            .bind(&p.account)
            .bind(&p.key)
            .bind(digest(host.token.as_bytes()))
            .bind(now())
            .execute(&self.0)
            .await?;
        Ok(())
    }
    pub async fn reserve_turn(
        &self,
        p: &Principal,
        id: &str,
        key: &str,
        limits: &Limits,
    ) -> Result<bool> {
        let mut tx = self.0.begin().await?;
        let existing: Option<String> = sqlx::query_scalar(
            "SELECT active_key FROM sessions WHERE id=? AND account=? AND state='owned'",
        )
        .bind(id)
        .bind(&p.account)
        .fetch_one(&mut *tx)
        .await?;
        if let Some(existing) = existing {
            return if existing == key {
                Err(Error::ambiguous())
            } else {
                Err(Error::capacity())
            };
        }
        let row = sqlx::query("SELECT count(active_key) AS active,coalesce(sum(retained_bytes),0) AS bytes FROM sessions WHERE account=? AND state!='deleted'")
            .bind(&p.account).fetch_one(&mut *tx).await?;
        if row.get::<i64, _>("active") >= i64::from(limits.active_turns_per_account)
            || (row.get::<i64, _>("bytes") as u64).saturating_add(limits.turn_reserve_bytes)
                > limits.retained_bytes_per_account
        {
            return Err(Error::capacity());
        }
        sqlx::query("UPDATE sessions SET active_key=?,active_since=?,retained_bytes=retained_bytes+?,changed_at=? WHERE id=?")
            .bind(key)
            .bind(now())
            .bind(limits.turn_reserve_bytes as i64)
            .bind(now())
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(true)
    }
    pub async fn finish_turn(&self, id: &str) -> Result<()> {
        sqlx::query(
            "UPDATE sessions SET active_key=NULL,active_since=NULL,changed_at=? WHERE id=?",
        )
        .bind(now())
        .bind(id)
        .execute(&self.0)
        .await?;
        Ok(())
    }
    pub async fn mark_deleting(&self, id: &str) -> Result<bool> {
        let state: String = sqlx::query_scalar("SELECT state FROM sessions WHERE id=?")
            .bind(id)
            .fetch_one(&self.0)
            .await?;
        if state == "deleted" {
            return Ok(false);
        }
        sqlx::query("UPDATE sessions SET state='deleting' WHERE id=?")
            .bind(id)
            .execute(&self.0)
            .await?;
        Ok(true)
    }
    pub async fn finish_delete(&self, id: &str) -> Result<()> {
        sqlx::query(
            "UPDATE sessions SET state='deleted',active_key=NULL,retained_bytes=0 WHERE id=?",
        )
        .bind(id)
        .execute(&self.0)
        .await?;
        Ok(())
    }
}
