use crate::{
    config::Limits,
    error::{Error, Result},
    identity::{Principal, digest, matches, random},
};
use sqlx::{PgPool, Row, postgres::PgPoolOptions};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone)]
pub struct Store(pub PgPool);
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
    pub async fn open(database_url: &str) -> anyhow::Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .acquire_timeout(Duration::from_secs(5))
            .connect(database_url)
            .await?;
        sqlx::migrate!().run(&pool).await?;
        Ok(Self(pool))
    }
    async fn lock_account(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        account: &str,
    ) -> Result<()> {
        sqlx::query("SELECT id FROM accounts WHERE id=$1 FOR UPDATE")
            .bind(account)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or_else(Error::missing)?;
        Ok(())
    }
    pub async fn principal(&self, token: &str) -> Result<Principal> {
        let row = sqlx::query("SELECT k.id,k.account FROM api_keys k JOIN accounts a ON a.id=k.account WHERE k.verifier=$1 AND k.active=1 AND a.active=1")
            .bind(digest(token.as_bytes())).fetch_optional(&self.0).await?.ok_or_else(Error::denied)?;
        Ok(Principal {
            account: row.get("account"),
            key: row.get("id"),
        })
    }
    pub async fn active(&self, p: &Principal) -> Result<()> {
        let active: i64 = sqlx::query_scalar("SELECT count(*) FROM api_keys k JOIN accounts a ON a.id=k.account WHERE k.id=$1 AND k.account=$2 AND k.active=1 AND a.active=1")
            .bind(&p.key).bind(&p.account).fetch_one(&self.0).await?;
        if active == 1 {
            Ok(())
        } else {
            Err(Error::denied())
        }
    }
    pub async fn host(&self, id: &str, token: &str) -> Result<Principal> {
        let row = sqlx::query("SELECT account,issuing_key,verifier FROM hosts WHERE id=$1")
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
            sqlx::query_scalar("SELECT issuing_key FROM hosts WHERE id=$1 AND account=$2")
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
            sqlx::query_scalar("SELECT state FROM sessions WHERE id=$1 AND account=$2")
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
            "SELECT id FROM sessions WHERE account=$1 AND state='owned' ORDER BY created,id",
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
        Self::lock_account(&mut tx, &p.account).await?;
        if let Some(row) = sqlx::query("SELECT fingerprint,state,result FROM claims WHERE account=$1 AND operation=$2 AND client_key=$3")
            .bind(&p.account).bind(operation).bind(key).fetch_optional(&mut *tx).await? {
            if row.get::<String,_>("fingerprint") != fingerprint { return Err(Error::conflict("operation key reused with different request")); }
            return if row.get::<String,_>("state") == "complete" {
                Ok(Claim::Complete(serde_json::from_str(row.get("result"))?))
            } else { Err(Error::ambiguous()) };
        }
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM claims WHERE account=$1")
            .bind(&p.account)
            .fetch_one(&mut *tx)
            .await?;
        if count >= i64::from(limits.claims_per_account) {
            return Err(Error::capacity());
        }
        if operation == "create" {
            let count: i64 = sqlx::query_scalar("SELECT (SELECT count(*) FROM sessions WHERE account=$1 AND state!='deleted') + (SELECT count(*) FROM claims WHERE account=$2 AND operation='create' AND state='pending')")
                .bind(&p.account).bind(&p.account).fetch_one(&mut *tx).await?;
            if count >= i64::from(limits.sessions_per_account) {
                return Err(Error::capacity());
            }
            let bytes: i64 = sqlx::query_scalar("SELECT coalesce(sum(retained_bytes),0)::bigint FROM sessions WHERE account=$1 AND state!='deleted'").bind(&p.account).fetch_one(&mut *tx).await?;
            if (bytes as u64)
                .saturating_add((count as u64 + 1).saturating_mul(limits.turn_reserve_bytes))
                > limits.retained_bytes_per_account
            {
                return Err(Error::capacity());
            }
        }
        let upstream = random("op");
        sqlx::query("INSERT INTO claims VALUES ($1,$2,$3,$4,$5,'pending',NULL,$6)")
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
            "INSERT INTO sessions(id,account,created,retained_bytes,changed_at) VALUES ($1,$2,$3,$4,$5)",
        )
        .bind(summary.session_id.as_str())
        .bind(&p.account)
        .bind(now())
        .bind(initial_bytes as i64)
        .bind(now())
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE claims SET state='complete',result=$1 WHERE upstream_key=$2 AND state='pending'",
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
        sqlx::query("INSERT INTO hosts VALUES ($1,$2,$3,$4,$5)")
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
        Self::lock_account(&mut tx, &p.account).await?;
        let existing: Option<String> = sqlx::query_scalar(
            "SELECT active_key FROM sessions WHERE id=$1 AND account=$2 AND state='owned'",
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
        let row = sqlx::query("SELECT count(active_key) AS active,coalesce(sum(retained_bytes),0)::bigint AS bytes FROM sessions WHERE account=$1 AND state!='deleted'")
            .bind(&p.account).fetch_one(&mut *tx).await?;
        if row.get::<i64, _>("active") >= i64::from(limits.active_turns_per_account)
            || (row.get::<i64, _>("bytes") as u64).saturating_add(limits.turn_reserve_bytes)
                > limits.retained_bytes_per_account
        {
            return Err(Error::capacity());
        }
        sqlx::query("UPDATE sessions SET active_key=$1,active_since=$2,retained_bytes=retained_bytes+$3,changed_at=$4 WHERE id=$5")
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
            "UPDATE sessions SET active_key=NULL,active_since=NULL,changed_at=$1 WHERE id=$2",
        )
        .bind(now())
        .bind(id)
        .execute(&self.0)
        .await?;
        Ok(())
    }
    pub async fn session_state(&self, id: &str) -> Result<String> {
        Ok(sqlx::query_scalar("SELECT state FROM sessions WHERE id=$1")
            .bind(id)
            .fetch_one(&self.0)
            .await?)
    }
    pub async fn mark_deleting(&self, id: &str) -> Result<bool> {
        let state: String = sqlx::query_scalar("SELECT state FROM sessions WHERE id=$1")
            .bind(id)
            .fetch_one(&self.0)
            .await?;
        if state == "deleted" {
            return Ok(false);
        }
        sqlx::query("UPDATE sessions SET state='deleting' WHERE id=$1")
            .bind(id)
            .execute(&self.0)
            .await?;
        Ok(true)
    }
    pub async fn finish_delete(&self, id: &str) -> Result<()> {
        sqlx::query(
            "UPDATE sessions SET state='deleted',active_key=NULL,retained_bytes=0 WHERE id=$1",
        )
        .bind(id)
        .execute(&self.0)
        .await?;
        Ok(())
    }
    pub async fn create_account(&self, limits: &Limits) -> Result<String> {
        let mut tx = self.0.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(8172301)")
            .execute(&mut *tx)
            .await?;
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM accounts")
            .fetch_one(&mut *tx)
            .await?;
        if count >= i64::from(limits.accounts) {
            return Err(Error::capacity());
        }
        let id = random("acct");
        sqlx::query("INSERT INTO accounts(id) VALUES ($1)")
            .bind(&id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(id)
    }
    pub async fn issue_key(&self, account: &str, limits: &Limits) -> Result<(String, String)> {
        self.issue_named_key(account, "API key", limits).await
    }
    pub async fn issue_named_key(
        &self,
        account: &str,
        name: &str,
        limits: &Limits,
    ) -> Result<(String, String)> {
        let mut tx = self.0.begin().await?;
        Self::lock_account(&mut tx, account).await?;
        let exists: i64 =
            sqlx::query_scalar("SELECT count(*) FROM accounts WHERE id=$1 AND active=1")
                .bind(account)
                .fetch_one(&mut *tx)
                .await?;
        if exists != 1 {
            return Err(Error::missing());
        }
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM api_keys WHERE account=$1")
            .bind(account)
            .fetch_one(&mut *tx)
            .await?;
        if count >= i64::from(limits.keys_per_account) {
            return Err(Error::capacity());
        }
        let id = random("key");
        let token = random("aex");
        sqlx::query(
            "INSERT INTO api_keys(id,account,verifier,name,prefix) VALUES ($1,$2,$3,$4,$5)",
        )
        .bind(&id)
        .bind(account)
        .bind(digest(token.as_bytes()))
        .bind(name)
        .bind(&token[..12])
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok((id, token))
    }
    pub async fn report_usage(
        &self,
        observed_at: i64,
        sessions: std::collections::BTreeMap<String, u64>,
    ) -> Result<()> {
        let mut tx = self.0.begin().await?;
        for (id, bytes) in sessions {
            let bytes =
                i64::try_from(bytes).map_err(|_| Error::invalid("storage size overflow"))?;
            sqlx::query("UPDATE sessions SET retained_bytes=$1 WHERE id=$2 AND state!='deleted' AND active_key IS NULL AND changed_at < $3")
                    .bind(bytes)
                    .bind(id)
                    .bind(observed_at)
                    .execute(&mut *tx)
                    .await?;
        }
        sqlx::query("INSERT INTO storage_report VALUES(1,$1) ON CONFLICT(singleton) DO UPDATE SET received=excluded.received").bind(observed_at).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn ready(&self) -> bool {
        sqlx::query("SELECT 1").execute(&self.0).await.is_ok()
    }
    pub async fn usage_received(&self) -> Result<Option<i64>> {
        Ok(
            sqlx::query_scalar("SELECT received FROM storage_report WHERE singleton=1")
                .fetch_optional(&self.0)
                .await?,
        )
    }
    pub async fn host_count(&self, account: &str) -> Result<i64> {
        Ok(
            sqlx::query_scalar("SELECT count(*) FROM hosts WHERE account=$1")
                .bind(account)
                .fetch_one(&self.0)
                .await?,
        )
    }
    pub async fn contains_session(&self, id: &str) -> Result<bool> {
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions WHERE id=$1")
            .bind(id)
            .fetch_one(&self.0)
            .await?;
        Ok(count != 0)
    }
    pub async fn expired_sessions(&self, retention_secs: u64) -> Result<Vec<String>> {
        Ok(sqlx::query_scalar(
            "SELECT id FROM sessions WHERE state='deleting' OR (state='owned' AND created < $1)",
        )
        .bind(now().saturating_sub(retention_secs as i64))
        .fetch_all(&self.0)
        .await?)
    }
    pub async fn revoke_key(&self, key: &str) -> Result<()> {
        sqlx::query("UPDATE api_keys SET active=0 WHERE id=$1")
            .bind(key)
            .execute(&self.0)
            .await?;
        Ok(())
    }
    pub async fn set_account_active(&self, account: &str, active: bool) -> Result<()> {
        sqlx::query("UPDATE accounts SET active=$1 WHERE id=$2")
            .bind(i64::from(active))
            .bind(account)
            .execute(&self.0)
            .await?;
        Ok(())
    }
    pub async fn resolve_create(&self, account: &str, client_key: &str) -> Result<()> {
        sqlx::query("UPDATE claims SET state='resolved' WHERE account=$1 AND client_key=$2 AND operation='create' AND state='pending'")
            .bind(account).bind(client_key).execute(&self.0).await?;
        Ok(())
    }
    pub async fn forget_host(&self, host: &str) -> Result<()> {
        sqlx::query("DELETE FROM hosts WHERE id=$1")
            .bind(host)
            .execute(&self.0)
            .await?;
        Ok(())
    }
    pub async fn inspect(&self) -> Result<(Vec<serde_json::Value>, Vec<serde_json::Value>)> {
        use serde_json::json;
        let accounts = sqlx::query("SELECT a.id,a.active,(SELECT count(*) FROM sessions s WHERE s.account=a.id AND s.state!='deleted') AS sessions,(SELECT count(active_key) FROM sessions s WHERE s.account=a.id AND s.state!='deleted') AS active_turns,(SELECT coalesce(sum(retained_bytes),0)::bigint FROM sessions s WHERE s.account=a.id AND s.state!='deleted') AS retained_bytes FROM accounts a").fetch_all(&self.0).await?.iter().map(|r| json!({"account":r.get::<String,_>("id"),"active":r.get::<i64,_>("active")==1,"sessions":r.get::<i64,_>("sessions"),"active_turns":r.get::<i64,_>("active_turns"),"retained_bytes":r.get::<i64,_>("retained_bytes")})).collect();
        let claims = sqlx::query("SELECT account,operation,client_key,created FROM claims WHERE state='pending'").fetch_all(&self.0).await?.iter().map(|r| json!({"account":r.get::<String,_>("account"),"operation":r.get::<String,_>("operation"),"client_key":r.get::<String,_>("client_key"),"created":r.get::<i64,_>("created")})).collect();
        Ok((accounts, claims))
    }
}
