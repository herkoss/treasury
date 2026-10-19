ALTER TABLE idempotency_keys ADD COLUMN claim UUID NOT NULL DEFAULT gen_random_uuid();
ALTER TABLE idempotency_keys ALTER COLUMN claim DROP DEFAULT;
