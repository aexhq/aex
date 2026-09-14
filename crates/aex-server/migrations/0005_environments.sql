CREATE TABLE environment_profiles (
    id TEXT PRIMARY KEY,
    document TEXT NOT NULL
);
CREATE TRIGGER environment_profiles_immutable BEFORE UPDATE OR DELETE ON environment_profiles
    FOR EACH ROW EXECUTE FUNCTION immutable_billing_record();
CREATE TABLE environment_grants (
    id TEXT PRIMARY KEY REFERENCES credit_reservations(id),
    operation TEXT NOT NULL REFERENCES claims(upstream_key),
    issuing_key TEXT NOT NULL REFERENCES api_keys(id),
    environment TEXT NOT NULL,
    profile TEXT NOT NULL REFERENCES environment_profiles(id),
    lifetime_ms BIGINT NOT NULL CHECK (lifetime_ms > 0),
    expires_at BIGINT NOT NULL,
    session TEXT,
    sandbox_id TEXT UNIQUE,
    UNIQUE(operation,environment)
);
