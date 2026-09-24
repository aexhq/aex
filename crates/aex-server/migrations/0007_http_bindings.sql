CREATE TABLE http_bindings (id TEXT PRIMARY KEY, document TEXT NOT NULL);
CREATE TRIGGER http_bindings_immutable BEFORE UPDATE OR DELETE ON http_bindings
    FOR EACH ROW EXECUTE FUNCTION immutable_billing_record();
CREATE TABLE http_grants (
    id TEXT PRIMARY KEY,
    account TEXT NOT NULL REFERENCES accounts(id),
    operation TEXT NOT NULL REFERENCES claims(upstream_key),
    issuing_key TEXT NOT NULL REFERENCES api_keys(id),
    environment TEXT NOT NULL,
    binding TEXT NOT NULL REFERENCES http_bindings(id),
    session TEXT,
    UNIQUE(operation,environment),
    UNIQUE(session,environment)
);
ALTER TABLE claims DROP CONSTRAINT claims_state_check;
ALTER TABLE claims ADD CONSTRAINT claims_state_check CHECK (state IN ('pending','complete','resolved','rejected'));
