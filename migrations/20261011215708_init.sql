-- Add migration script here
CREATE TABLE assets (
    code        TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    scale       SMALLINT NOT NULL CHECK (scale BETWEEN 0 AND 9),
    archived_at TIMESTAMPTZ,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE accounts (
    id         UUID PRIMARY KEY,
    name       TEXT NOT NULL,
    key_hash   BYTEA NOT NULL UNIQUE,
    is_issuer  BOOLEAN NOT NULL DEFAULT false,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX accounts_one_issuer ON accounts (is_issuer) WHERE is_issuer;

CREATE TABLE proposals (
    id          UUID PRIMARY KEY,
    proposer_id UUID NOT NULL REFERENCES accounts (id),
    memo        TEXT NOT NULL DEFAULT '',
    state       TEXT NOT NULL
                CHECK (state IN ('pending', 'settled', 'rejected', 'cancelled', 'expired')),
    expires_at  TIMESTAMPTZ NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    closed_at   TIMESTAMPTZ
);

CREATE TABLE legs (
    proposal_id UUID NOT NULL REFERENCES proposals (id),
    position    SMALLINT NOT NULL,
    from_id     UUID NOT NULL REFERENCES accounts (id),
    to_id       UUID NOT NULL REFERENCES accounts (id),
    asset_code  TEXT NOT NULL REFERENCES assets (code),
    amount      BIGINT NOT NULL CHECK (amount > 0),
    PRIMARY KEY (proposal_id, position),
    CHECK (from_id <> to_id)
);

CREATE INDEX legs_from ON legs (from_id);

CREATE TABLE approvals (
    proposal_id UUID NOT NULL REFERENCES proposals (id),
    account_id  UUID NOT NULL REFERENCES accounts (id),
    decision    TEXT NOT NULL CHECK (decision IN ('approved', 'rejected')),
    decided_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (proposal_id, account_id)
);

CREATE TABLE entries (
    id          BIGSERIAL PRIMARY KEY,
    proposal_id UUID NOT NULL REFERENCES proposals (id),
    account_id  UUID NOT NULL REFERENCES accounts (id),
    asset_code  TEXT NOT NULL REFERENCES assets (code),
    amount      BIGINT NOT NULL CHECK (amount <> 0),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX entries_account_asset ON entries (account_id, asset_code);

CREATE FUNCTION forbid_change() RETURNS trigger AS $$
BEGIN
    RAISE EXCEPTION 'entries are append-only';
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER entries_append_only
    BEFORE UPDATE OR DELETE ON entries
    FOR EACH ROW EXECUTE FUNCTION forbid_change();
