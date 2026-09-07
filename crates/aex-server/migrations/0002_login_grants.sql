CREATE TABLE login_grants (
    verifier TEXT PRIMARY KEY,
    session_verifier TEXT NOT NULL REFERENCES dashboard_sessions(verifier) ON DELETE CASCADE,
    challenge TEXT NOT NULL,
    redirect_uri TEXT NOT NULL,
    expires BIGINT NOT NULL
);
CREATE INDEX login_grants_session ON login_grants(session_verifier);
