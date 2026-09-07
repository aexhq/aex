CREATE TABLE accounts (
    id TEXT PRIMARY KEY,
    active BIGINT NOT NULL DEFAULT 1 CHECK (active IN (0,1))
);
CREATE TABLE api_keys (
    id TEXT PRIMARY KEY,
    account TEXT NOT NULL REFERENCES accounts(id),
    verifier TEXT NOT NULL UNIQUE,
    active BIGINT NOT NULL DEFAULT 1 CHECK (active IN (0,1))
);
CREATE INDEX keys_account ON api_keys(account);
CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    account TEXT NOT NULL REFERENCES accounts(id),
    state TEXT NOT NULL DEFAULT 'owned' CHECK (state IN ('owned','deleting','deleted')),
    created BIGINT NOT NULL,
    active_key TEXT,
    active_since BIGINT,
    retained_bytes BIGINT NOT NULL DEFAULT 0,
    changed_at BIGINT NOT NULL DEFAULT 0
);
CREATE INDEX sessions_account ON sessions(account);
CREATE TABLE hosts (
    id TEXT PRIMARY KEY,
    account TEXT NOT NULL REFERENCES accounts(id),
    issuing_key TEXT NOT NULL REFERENCES api_keys(id),
    verifier TEXT NOT NULL,
    created BIGINT NOT NULL
);
CREATE INDEX hosts_account ON hosts(account);
CREATE TABLE claims (
    account TEXT NOT NULL REFERENCES accounts(id),
    operation TEXT NOT NULL,
    client_key TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    upstream_key TEXT NOT NULL UNIQUE,
    state TEXT NOT NULL CHECK (state IN ('pending','complete','resolved')),
    result TEXT,
    created BIGINT NOT NULL,
    PRIMARY KEY (account, operation, client_key)
);
CREATE TABLE storage_report (singleton BIGINT PRIMARY KEY CHECK (singleton=1), received BIGINT NOT NULL);

ALTER TABLE accounts ADD COLUMN identity_subject TEXT UNIQUE;
ALTER TABLE accounts ADD COLUMN email TEXT;
ALTER TABLE accounts ADD COLUMN created BIGINT NOT NULL DEFAULT extract(epoch FROM now())::bigint;
ALTER TABLE api_keys ADD COLUMN name TEXT NOT NULL DEFAULT 'API key';
ALTER TABLE api_keys ADD COLUMN prefix TEXT NOT NULL DEFAULT '';
ALTER TABLE api_keys ADD COLUMN created BIGINT NOT NULL DEFAULT extract(epoch FROM now())::bigint;
CREATE TABLE dashboard_sessions (
    verifier TEXT PRIMARY KEY,
    account TEXT NOT NULL REFERENCES accounts(id),
    expires BIGINT NOT NULL
);
CREATE INDEX dashboard_sessions_account ON dashboard_sessions(account);
CREATE TABLE artifacts (
    account TEXT NOT NULL REFERENCES accounts(id),
    kind TEXT NOT NULL CHECK (kind IN ('agentloops','tools')),
    id TEXT NOT NULL,
    bytes BIGINT NOT NULL CHECK (bytes > 0),
    PRIMARY KEY (account,kind,id)
);
