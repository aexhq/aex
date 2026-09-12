CREATE TABLE attachments (
    id TEXT PRIMARY KEY,
    account TEXT NOT NULL REFERENCES accounts(id),
    session TEXT NOT NULL REFERENCES sessions(id),
    object_key TEXT NOT NULL UNIQUE,
    content_type TEXT NOT NULL CHECK (content_type IN ('application/pdf','image/png','image/jpeg','image/gif','image/webp')),
    bytes BIGINT NOT NULL CHECK (bytes > 0),
    expires_at BIGINT NOT NULL,
    read_verifier TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('pending','ready','deleting')),
    operation_key TEXT NOT NULL UNIQUE REFERENCES claims(upstream_key)
);
CREATE INDEX attachments_account ON attachments(account);
CREATE INDEX attachments_session ON attachments(session);
CREATE INDEX attachments_expiry ON attachments(expires_at);
