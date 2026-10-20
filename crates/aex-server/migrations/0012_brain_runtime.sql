DO $$ BEGIN
 IF EXISTS(SELECT 1 FROM sessions WHERE active_key IS NOT NULL)
 OR EXISTS(SELECT 1 FROM model_usage WHERE NOT terminal)
 OR EXISTS(SELECT 1 FROM credit_reservations WHERE state='open' AND meter IN ('turn_ms','sandbox_ms'))
 OR EXISTS(SELECT 1 FROM claims WHERE state='pending')
 THEN RAISE EXCEPTION 'Drain and reconcile old runtime work and compute reservations before Brain cutover'; END IF;
END $$;
ALTER TABLE sessions ADD COLUMN runtime TEXT NOT NULL DEFAULT 'retained';
ALTER TABLE sessions ADD COLUMN brain_cursor TEXT NOT NULL DEFAULT '';
UPDATE sessions SET model_complete=TRUE,model_pending=0 WHERE runtime='retained';
ALTER TABLE sessions ALTER COLUMN runtime SET DEFAULT 'brain';

CREATE TABLE executors (
 id TEXT PRIMARY KEY, account TEXT NOT NULL REFERENCES accounts(id),
 issuing_key TEXT NOT NULL REFERENCES api_keys(id), verifier TEXT NOT NULL,
 session TEXT REFERENCES sessions(id), browser_grant TEXT, created BIGINT NOT NULL
);
CREATE INDEX executors_account ON executors(account);
CREATE TABLE browser_grants (
 id TEXT PRIMARY KEY, verifier TEXT NOT NULL UNIQUE,
 account TEXT NOT NULL REFERENCES accounts(id), issuing_key TEXT NOT NULL REFERENCES api_keys(id),
 origin TEXT NOT NULL, expires_at BIGINT NOT NULL, active BOOLEAN NOT NULL DEFAULT TRUE,
 session TEXT REFERENCES sessions(id), composition TEXT NOT NULL, tools TEXT NOT NULL
);
ALTER TABLE executors ADD CONSTRAINT executors_browser_grant FOREIGN KEY(browser_grant) REFERENCES browser_grants(id);
CREATE TABLE brain_turns (
 operation TEXT PRIMARY KEY REFERENCES claims(upstream_key), session TEXT NOT NULL REFERENCES sessions(id),
 since_cursor TEXT NOT NULL, input_id TEXT, accepted TEXT, started_ms BIGINT NOT NULL,
 outcome TEXT NOT NULL DEFAULT 'pending'
);
CREATE TABLE brain_model_usage (
 session TEXT NOT NULL REFERENCES sessions(id), request_id TEXT NOT NULL, attempt_id TEXT NOT NULL,
 account TEXT NOT NULL REFERENCES accounts(id), pricebook TEXT REFERENCES pricebooks(id),
 started_ms BIGINT NOT NULL, terminal BOOLEAN NOT NULL DEFAULT FALSE,
 complete BOOLEAN NOT NULL DEFAULT FALSE, evidence TEXT NOT NULL,
 input_tokens BIGINT, output_tokens BIGINT, rated BIGINT NOT NULL DEFAULT 0, charged BIGINT NOT NULL DEFAULT 0,
 PRIMARY KEY(session,request_id,attempt_id)
);
CREATE INDEX brain_model_usage_account ON brain_model_usage(account);
CREATE TABLE tool_grants (
 id TEXT PRIMARY KEY, account TEXT NOT NULL REFERENCES accounts(id), issuing_key TEXT NOT NULL REFERENCES api_keys(id),
 operation TEXT NOT NULL REFERENCES claims(upstream_key), session TEXT REFERENCES sessions(id),
 alias TEXT NOT NULL, provider TEXT NOT NULL, parameters TEXT NOT NULL, profile TEXT, expires_ms BIGINT NOT NULL,
 revoked BOOLEAN NOT NULL DEFAULT FALSE, settled BOOLEAN NOT NULL DEFAULT FALSE,
 UNIQUE(operation,alias)
);
CREATE TABLE tool_service_jobs (id TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE INDEX tool_service_jobs_completion ON tool_service_jobs ((value::jsonb->>'verifier'));
CREATE INDEX tool_service_jobs_grant ON tool_service_jobs ((value::jsonb->>'grant'));
