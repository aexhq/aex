CREATE TABLE client_grants (
    id TEXT PRIMARY KEY,
    verifier TEXT UNIQUE NOT NULL,
    account TEXT NOT NULL REFERENCES accounts(id),
    issuing_key TEXT NOT NULL REFERENCES api_keys(id),
    host TEXT UNIQUE NOT NULL REFERENCES hosts(id),
    origin TEXT NOT NULL,
    expires_at BIGINT NOT NULL,
    fingerprint TEXT NOT NULL,
    components TEXT NOT NULL,
    session TEXT REFERENCES sessions(id)
);
