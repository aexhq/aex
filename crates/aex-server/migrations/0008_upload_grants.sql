ALTER TABLE attachments ADD COLUMN metadata TEXT;
UPDATE attachments SET metadata=c.result FROM claims c
    WHERE attachments.operation_key=c.upstream_key AND attachments.state='ready' AND c.state='complete';
CREATE TABLE attachment_upload_grants (
    attachment TEXT PRIMARY KEY REFERENCES attachments(id) ON DELETE CASCADE,
    issuing_key TEXT NOT NULL REFERENCES api_keys(id),
    verifier TEXT NOT NULL,
    expires_at BIGINT NOT NULL,
    digest TEXT
);
