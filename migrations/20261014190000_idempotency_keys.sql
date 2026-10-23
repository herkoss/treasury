CREATE TABLE idempotency_keys (
    account_id  UUID NOT NULL REFERENCES accounts (id),
    key         TEXT NOT NULL,
    fingerprint BYTEA NOT NULL,
    status      INTEGER,
    body        BYTEA,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (account_id, key)
);
