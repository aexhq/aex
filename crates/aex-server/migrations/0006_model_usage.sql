ALTER TABLE sessions ADD COLUMN model_cursor BIGINT NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN model_complete BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE sessions ADD COLUMN model_observed_at BIGINT NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN model_pending BIGINT NOT NULL DEFAULT 0 CHECK (model_pending >= 0);

CREATE TABLE model_price_history (
    id BIGSERIAL PRIMARY KEY,
    account TEXT NOT NULL REFERENCES wallets(account),
    pricebook TEXT NOT NULL REFERENCES pricebooks(id),
    effective_ms BIGINT NOT NULL
);
CREATE INDEX model_price_history_account ON model_price_history(account,effective_ms DESC,id DESC);
INSERT INTO model_price_history(account,pricebook,effective_ms)
    SELECT account,pricebook,accepted_at*1000 FROM wallets;
CREATE TRIGGER model_price_history_immutable BEFORE UPDATE OR DELETE ON model_price_history
    FOR EACH ROW EXECUTE FUNCTION immutable_billing_record();

CREATE TABLE model_usage (
    session TEXT NOT NULL REFERENCES sessions(id),
    sequence BIGINT NOT NULL,
    account TEXT NOT NULL REFERENCES accounts(id),
    pricebook TEXT REFERENCES pricebooks(id),
    started_ms BIGINT NOT NULL,
    input_bytes BIGINT NOT NULL DEFAULT 0,
    media_inputs BIGINT NOT NULL DEFAULT 0,
    output_bytes BIGINT NOT NULL DEFAULT 0,
    terminal BOOLEAN NOT NULL DEFAULT FALSE,
    complete BOOLEAN NOT NULL DEFAULT FALSE,
    usage TEXT NOT NULL DEFAULT '{}',
    input_tokens BIGINT,
    output_tokens BIGINT,
    rated BIGINT NOT NULL DEFAULT 0,
    charged BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (session,sequence)
);
CREATE INDEX model_usage_account ON model_usage(account);
CREATE TABLE model_usage_totals (
    account TEXT NOT NULL REFERENCES wallets(account),
    pricebook TEXT NOT NULL REFERENCES pricebooks(id),
    tokens BIGINT NOT NULL DEFAULT 0,
    rated BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (account,pricebook)
);
