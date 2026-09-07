CREATE TABLE accounts (
    id TEXT PRIMARY KEY,
    active INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0,1))
);
CREATE TABLE api_keys (
    id TEXT PRIMARY KEY,
    account TEXT NOT NULL REFERENCES accounts(id),
    verifier TEXT NOT NULL UNIQUE,
    active INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0,1))
);
CREATE INDEX keys_account ON api_keys(account);
CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    account TEXT NOT NULL REFERENCES accounts(id),
    state TEXT NOT NULL DEFAULT 'owned' CHECK (state IN ('owned','deleting','deleted')),
    created INTEGER NOT NULL,
    active_key TEXT,
    active_since INTEGER,
    retained_bytes INTEGER NOT NULL DEFAULT 0,
    changed_at INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX sessions_account ON sessions(account);
CREATE TABLE hosts (
    id TEXT PRIMARY KEY,
    account TEXT NOT NULL REFERENCES accounts(id),
    issuing_key TEXT NOT NULL REFERENCES api_keys(id),
    verifier TEXT NOT NULL,
    created INTEGER NOT NULL
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
    created INTEGER NOT NULL,
    PRIMARY KEY (account, operation, client_key)
);
CREATE TABLE storage_report (singleton INTEGER PRIMARY KEY CHECK (singleton=1), received INTEGER NOT NULL);
