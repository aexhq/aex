CREATE TABLE pricebooks (
    id TEXT PRIMARY KEY,
    document TEXT NOT NULL,
    created BIGINT NOT NULL
);
CREATE TABLE wallets (
    account TEXT PRIMARY KEY REFERENCES accounts(id),
    pricebook TEXT NOT NULL REFERENCES pricebooks(id),
    accepted_at BIGINT NOT NULL,
    balance BIGINT NOT NULL DEFAULT 0,
    reserved BIGINT NOT NULL DEFAULT 0 CHECK (reserved >= 0),
    spend_limit BIGINT NOT NULL CHECK (spend_limit >= 0),
    suspended BOOLEAN NOT NULL DEFAULT FALSE
);
CREATE TABLE credit_ledger (
    id BIGSERIAL PRIMARY KEY,
    account TEXT NOT NULL REFERENCES wallets(account),
    kind TEXT NOT NULL CHECK (kind IN ('topup','usage','refund','reversal','adjustment','price_acceptance')),
    reference TEXT NOT NULL,
    delta BIGINT NOT NULL,
    description TEXT NOT NULL,
    created BIGINT NOT NULL,
    UNIQUE (account,kind,reference)
);
CREATE INDEX credit_ledger_account ON credit_ledger(account,id DESC);
CREATE FUNCTION immutable_billing_record() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'billing history is append-only';
END;
$$;
CREATE TRIGGER pricebooks_immutable BEFORE UPDATE OR DELETE ON pricebooks
    FOR EACH ROW EXECUTE FUNCTION immutable_billing_record();
CREATE TRIGGER credit_ledger_immutable BEFORE UPDATE OR DELETE ON credit_ledger
    FOR EACH ROW EXECUTE FUNCTION immutable_billing_record();

CREATE TABLE credit_reservations (
    id TEXT PRIMARY KEY,
    account TEXT NOT NULL REFERENCES wallets(account),
    resource TEXT NOT NULL,
    meter TEXT NOT NULL,
    pricebook TEXT NOT NULL REFERENCES pricebooks(id),
    max_units BIGINT NOT NULL CHECK (max_units >= 0),
    units BIGINT NOT NULL DEFAULT 0 CHECK (units >= 0 AND units <= max_units),
    rated BIGINT NOT NULL DEFAULT 0 CHECK (rated >= 0),
    remaining BIGINT NOT NULL CHECK (remaining >= 0),
    state TEXT NOT NULL CHECK (state IN ('open','closed')),
    created BIGINT NOT NULL
);
CREATE INDEX credit_reservations_resource ON credit_reservations(resource,state);
CREATE TABLE metered_usage (
    id TEXT PRIMARY KEY,
    reservation TEXT NOT NULL REFERENCES credit_reservations(id),
    units BIGINT NOT NULL CHECK (units >= 0),
    terminal BOOLEAN NOT NULL,
    delta BIGINT NOT NULL CHECK (delta >= 0),
    created BIGINT NOT NULL
);
CREATE TABLE session_turns (
    operation TEXT PRIMARY KEY REFERENCES claims(upstream_key),
    session TEXT NOT NULL REFERENCES sessions(id),
    since_sequence BIGINT NOT NULL CHECK (since_sequence >= 0)
);
ALTER TABLE attachments ADD COLUMN published_at BIGINT;
CREATE TABLE attachment_downloads (
    id TEXT PRIMARY KEY,
    reservation TEXT NOT NULL REFERENCES credit_reservations(id),
    bytes BIGINT NOT NULL CHECK (bytes > 0),
    received BIGINT NOT NULL DEFAULT 0 CHECK (received >= 0 AND received <= bytes),
    expires BIGINT NOT NULL
);
CREATE INDEX attachment_downloads_reservation ON attachment_downloads(reservation,expires);
CREATE TRIGGER metered_usage_immutable BEFORE UPDATE OR DELETE ON metered_usage
    FOR EACH ROW EXECUTE FUNCTION immutable_billing_record();

CREATE TABLE topups (
    id TEXT PRIMARY KEY,
    account TEXT NOT NULL REFERENCES wallets(account),
    client_key TEXT NOT NULL,
    amount_cents BIGINT NOT NULL CHECK (amount_cents > 0),
    state TEXT NOT NULL CHECK (state IN ('creating','open','paid','expired','failed')),
    checkout_id TEXT UNIQUE,
    checkout_url TEXT,
    payment_intent TEXT UNIQUE,
    receipt_url TEXT,
    refunded_cents BIGINT NOT NULL DEFAULT 0 CHECK (refunded_cents >= 0 AND refunded_cents <= amount_cents),
    created BIGINT NOT NULL,
    UNIQUE (account,client_key)
);
CREATE TABLE credit_refunds (
    id TEXT PRIMARY KEY,
    account TEXT NOT NULL REFERENCES wallets(account),
    topup TEXT NOT NULL REFERENCES topups(id),
    client_key TEXT NOT NULL,
    amount_cents BIGINT NOT NULL CHECK (amount_cents > 0),
    state TEXT NOT NULL CHECK (state IN ('creating','pending','succeeded','failed','canceled')),
    provider_id TEXT UNIQUE,
    created BIGINT NOT NULL,
    UNIQUE (account,client_key)
);
CREATE TABLE payment_events (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    created BIGINT NOT NULL
);
CREATE TABLE payment_disputes (
    id TEXT PRIMARY KEY,
    account TEXT NOT NULL REFERENCES wallets(account),
    amount BIGINT NOT NULL CHECK (amount > 0),
    withdrawn BOOLEAN NOT NULL DEFAULT FALSE,
    restored BOOLEAN NOT NULL DEFAULT FALSE
);
