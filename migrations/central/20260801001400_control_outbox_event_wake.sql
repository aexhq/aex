-- aex-migration: tx=yes destructive=no phase=expand
-- One accepted asynchronous invocation per INSERT statement replaces both the
-- queue wakeup and the periodic outbox scan. The invocation is deliberately
-- issued before commit: its payload carries the exact transaction id, so the
-- worker can distinguish the pre-commit race from a rollback.

CREATE TABLE control.outbox_wake_target (
    singleton  boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    lambda_arn text NOT NULL CHECK (
        lambda_arn ~ '^arn:aws[a-z-]*:lambda:[a-z0-9-]+:[0-9]{12}:function:[A-Za-z0-9_-]+:[A-Za-z0-9_-]+$'
    )
);

CREATE FUNCTION control.invoke_outbox_wake()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
DECLARE
    target_arn text;
    anchor_id uuid;
    transaction_id xid8;
    wake_payload json;
    acceptance_status integer;
BEGIN
    SELECT min(inserted.id) INTO anchor_id FROM inserted_outbox AS inserted;
    IF anchor_id IS NULL THEN
        RETURN NULL;
    END IF;

    -- Local PostgreSQL deliberately has no aws_lambda extension. That is the
    -- only disabled state: once the extension schema exists, an absent target
    -- is hosted-plane misconfiguration and must abort the producer's INSERT.
    IF to_regnamespace('aws_lambda') IS NULL THEN
        RETURN NULL;
    END IF;

    SELECT configured.lambda_arn
      INTO target_arn
      FROM control.outbox_wake_target AS configured
     WHERE configured.singleton;
    IF target_arn IS NULL THEN
        RAISE EXCEPTION 'central control wake target is not configured';
    END IF;

    transaction_id := pg_current_xact_id();
    wake_payload := json_build_object(
        'schema', 'aex.control-outbox-wake.v1',
        'anchorId', anchor_id,
        'transactionId', transaction_id::text
    );

    -- Dynamic SQL keeps the canonical migration portable to localhost. On an
    -- enabled hosted plane any missing extension, IAM denial, network failure,
    -- or non-202 acceptance error escapes this function and aborts the INSERT
    -- transaction. Event execution itself is retried by Lambda.
    EXECUTE
        'SELECT status_code FROM aws_lambda.invoke($1::text, $2::json, NULL::text, ''Event''::text)'
        INTO acceptance_status
        USING target_arn, wake_payload;
    IF acceptance_status <> 202 THEN
        RAISE EXCEPTION 'central control wake was not accepted asynchronously';
    END IF;

    RETURN NULL;
END;
$$;

CREATE TRIGGER outbox_message_event_wake
AFTER INSERT ON control.outbox_message
REFERENCING NEW TABLE AS inserted_outbox
FOR EACH STATEMENT
EXECUTE FUNCTION control.invoke_outbox_wake();

CREATE INDEX outbox_dispatched_retention_ix
    ON control.outbox_message (dispatched_at, id)
    WHERE dispatched_at IS NOT NULL;
