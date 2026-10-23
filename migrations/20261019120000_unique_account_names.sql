-- Names are unique, case-insensitively. Rows that already collide keep the
-- oldest one as is and get an id prefix appended to the rest.
UPDATE accounts a
SET name = a.name || '-' || left(a.id::text, 8)
WHERE EXISTS (
    SELECT 1 FROM accounts b
    WHERE lower(b.name) = lower(a.name)
      AND (b.created_at, b.id) < (a.created_at, a.id)
);

CREATE UNIQUE INDEX accounts_name ON accounts (lower(name));
