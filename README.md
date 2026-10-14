# treasury

An asset ledger with consent-based settlement.

> Alice does not *execute* a swap, she *offers* one. Her funds are reserved the moment she offers;
> Bob's funds move only when Bob approves; and the balances change only at settlement, all legs at
> once or none at all.

The full proposal, with the invariants this service protects, lives in
[`docs/proposal.md`](docs/proposal.md). The entities and their lifecycle are in
[`docs/domain.md`](docs/domain.md).

## Run it

```sh
cp .env.example .env            # set OPERATOR_KEY to any long random string
docker compose up --build       # Postgres on :5433, the API on :8888
```

Without Docker for the app itself: `docker compose up db`, then `cargo run --bin server`.
Migrations run at start-up. The `@issuer` account is created (or its key rotated) from
`OPERATOR_KEY` every time the server starts.

Every request except `GET /health` carries `X-Account-Key`. The operator key acts as `@issuer`;
an account key, returned once by `POST /accounts`, acts as that account.

## API

| Method | Path | Who | What |
|---|---|---|---|
| `POST` | `/assets` | issuer | create `{code, name, scale}` → `201` |
| `GET` | `/assets` | anyone | list |
| `GET` | `/assets/{code}` | anyone | read |
| `PATCH` | `/assets/{code}` | issuer | rename `{name}` |
| `DELETE` | `/assets/{code}` | issuer | archive, `409` while balances or pending offers exist |
| `POST` | `/accounts` | issuer | create `{name}` → `201 {id, name, key}`, the key is shown once |
| `GET` | `/accounts/{id}/balances` | owner, issuer | `posted`, `on_hold`, `available` per asset |
| `GET` | `/accounts/{id}/entries` | owner, issuer | journal, keyset paginated with `?after=<id>&limit=<n>` |
| `GET` | `/accounts/{id}/inbox` | owner, issuer | pending offers that await this account's vote |
| `GET` | `/accounts/{id}/outbox` | owner, issuer | pending offers this account proposed |
| `POST` | `/transactions` | anyone | propose → `201` if it settled at once, `202` if it waits for votes |
| `GET` | `/transactions/{id}` | parties, issuer | state, legs, every required approval and its timestamp |
| `POST` | `/transactions/{id}/approve` | a debited party | vote yes, the last vote settles |
| `POST` | `/transactions/{id}/reject` | a debited party | vote no, every hold is released |
| `POST` | `/transactions/{id}/cancel` | proposer | withdraw a pending offer |

`{id}` in account paths is a UUID or `@issuer`. Amounts are decimal strings, parsed and
rendered with the asset's `scale`. Errors are `application/problem+json`.

One request shape covers deposit, withdrawal, transfer and swap; only the legs differ:

```json
{
  "memo": "100 USD for 1.5 XAU",
  "expires_in_secs": 900,
  "legs": [
    { "from": "<alice>", "to": "<bob>",   "asset": "USD", "amount": "100.00" },
    { "from": "<bob>",   "to": "<alice>", "asset": "XAU", "amount": "1.5" }
  ]
}
```

Required approvals are derived: the distinct `from` accounts. The proposer is approved
implicitly, so a plain transfer settles in one call and a swap waits for the other side.

## Walkthrough

The demo script from the proposal, as `curl`. `jq` is used only to pull ids and keys out of
responses.

```sh
API=localhost:8888
OP=$(grep OPERATOR_KEY .env | cut -d= -f2- | tr -d '"')
J='content-type: application/json'

# 1. two assets, two accounts
curl -s -X POST $API/assets -H "x-account-key: $OP" -H "$J" -d '{"code":"USD","name":"US Dollar","scale":2}'
curl -s -X POST $API/assets -H "x-account-key: $OP" -H "$J" -d '{"code":"XAU","name":"Gold, troy ounces","scale":3}'
ALICE=$(curl -s -X POST $API/accounts -H "x-account-key: $OP" -H "$J" -d '{"name":"alice"}')
BOB=$(curl -s -X POST $API/accounts -H "x-account-key: $OP" -H "$J" -d '{"name":"bob"}')
ALICE_ID=$(echo $ALICE | jq -r .id); ALICE_KEY=$(echo $ALICE | jq -r .key)
BOB_ID=$(echo $BOB | jq -r .id);     BOB_KEY=$(echo $BOB | jq -r .key)

# 2. deposits from @issuer settle at once
curl -s -X POST $API/transactions -H "x-account-key: $OP" -H "$J" \
  -d "{\"legs\":[{\"from\":\"@issuer\",\"to\":\"$ALICE_ID\",\"asset\":\"USD\",\"amount\":\"1000\"}]}"
curl -s -X POST $API/transactions -H "x-account-key: $OP" -H "$J" \
  -d "{\"legs\":[{\"from\":\"@issuer\",\"to\":\"$BOB_ID\",\"asset\":\"XAU\",\"amount\":\"10\"}]}"

# 3. a transfer debits only Alice: one required approval, hers, implicit -> 201 settled
curl -s -X POST $API/transactions -H "x-account-key: $ALICE_KEY" -H "$J" \
  -d "{\"legs\":[{\"from\":\"$ALICE_ID\",\"to\":\"$BOB_ID\",\"asset\":\"USD\",\"amount\":\"100\"}]}"
curl -s $API/accounts/$ALICE_ID/balances -H "x-account-key: $ALICE_KEY"

# 4. an unfair offer: 202, nothing moves, Bob is pending, Alice's 1 USD is on hold
OFFER=$(curl -s -X POST $API/transactions -H "x-account-key: $ALICE_KEY" -H "$J" \
  -d "{\"memo\":\"1 USD for 100 XAU\",\"legs\":[
        {\"from\":\"$ALICE_ID\",\"to\":\"$BOB_ID\",\"asset\":\"USD\",\"amount\":\"1\"},
        {\"from\":\"$BOB_ID\",\"to\":\"$ALICE_ID\",\"asset\":\"XAU\",\"amount\":\"100\"}]}")
OFFER_ID=$(echo $OFFER | jq -r .id)

# 5. Alice cannot vote for Bob: her own vote is already recorded -> 409; an outsider -> 403
curl -s -X POST $API/transactions/$OFFER_ID/approve -H "x-account-key: $ALICE_KEY"

# 6. Bob reads his inbox and rejects: every hold is released, his XAU untouched
curl -s $API/accounts/$BOB_ID/inbox -H "x-account-key: $BOB_KEY"
curl -s -X POST $API/transactions/$OFFER_ID/reject -H "x-account-key: $BOB_KEY"

# 7. a fair swap, Bob approves: both legs move, or neither
SWAP=$(curl -s -X POST $API/transactions -H "x-account-key: $ALICE_KEY" -H "$J" \
  -d "{\"legs\":[
        {\"from\":\"$ALICE_ID\",\"to\":\"$BOB_ID\",\"asset\":\"USD\",\"amount\":\"100\"},
        {\"from\":\"$BOB_ID\",\"to\":\"$ALICE_ID\",\"asset\":\"XAU\",\"amount\":\"1.5\"}]}")
SWAP_ID=$(echo $SWAP | jq -r .id)
curl -s -X POST $API/transactions/$SWAP_ID/approve -H "x-account-key: $BOB_KEY"

# 8. approving again -> 409, balances unchanged
curl -s -X POST $API/transactions/$SWAP_ID/approve -H "x-account-key: $BOB_KEY"

# 9. a short-lived offer expires on its own and releases the hold
curl -s -X POST $API/transactions -H "x-account-key: $ALICE_KEY" -H "$J" \
  -d "{\"expires_in_secs\":1,\"legs\":[
        {\"from\":\"$ALICE_ID\",\"to\":\"$BOB_ID\",\"asset\":\"USD\",\"amount\":\"50\"},
        {\"from\":\"$BOB_ID\",\"to\":\"$ALICE_ID\",\"asset\":\"XAU\",\"amount\":\"1\"}]}" | jq -r .id
sleep 2; curl -s $API/transactions/<that id> -H "x-account-key: $ALICE_KEY"

# 10. funds on hold cannot be promised twice -> 422 insufficient available balance
curl -s -X POST $API/transactions -H "x-account-key: $ALICE_KEY" -H "$J" \
  -d "{\"legs\":[{\"from\":\"$ALICE_ID\",\"to\":\"$BOB_ID\",\"asset\":\"USD\",\"amount\":\"600\"},
                 {\"from\":\"$BOB_ID\",\"to\":\"$ALICE_ID\",\"asset\":\"XAU\",\"amount\":\"1\"}]}"
curl -s -X POST $API/transactions -H "x-account-key: $ALICE_KEY" -H "$J" \
  -d "{\"legs\":[{\"from\":\"$ALICE_ID\",\"to\":\"$BOB_ID\",\"asset\":\"USD\",\"amount\":\"600\"}]}"

# 11. the audit trail: every entry names the proposal that authorised it
curl -s $API/accounts/$ALICE_ID/entries -H "x-account-key: $ALICE_KEY"
```

## Design notes

**Schema.** Six tables: `assets`, `accounts`, `proposals`, `legs`, `approvals`, `entries`.
`entries` is the journal; a trigger forbids `UPDATE` and `DELETE` on it (I3). The `@issuer`
account is the nil UUID and the only row with `is_issuer`, enforced by a partial unique index.

**Holds and balances are derived, never stored.** A hold is "a leg whose `from` account has an
`approved` row on a proposal that is still `pending` and not past `expires_at`". `posted` is
`SUM(entries)`, `available` is `posted − on_hold`. There is no hold table to keep in sync, so
"a hold is released exactly once" (I6) is true by construction: it disappears the instant the
proposal leaves `pending`, whatever the reason.

**Expiry is lazy.** No background sweeper. Every query that cares reads
`state = 'pending' AND expires_at > now()`, and the API reports such a proposal as `expired`.
A vote on it is refused with `409`.

**Settlement is one database transaction.** Lock the proposal row, lock every account in the
legs with `SELECT … FOR UPDATE ORDER BY id` (a fixed order, so two settlements can never
deadlock), check the voter's available balance, record the vote, and when it was the last one,
insert all journal entries with a single `INSERT … SELECT` and close the proposal.
`READ COMMITTED` is enough because every balance check runs after the row locks are held.

**Consent (I4) and exactly-once (I5).** The domain module (`src/domain.rs`) is pure: it knows
nothing about HTTP or SQL. `Proposal::vote` decides from facts (state, proposer, required set,
approved set) whether a vote is allowed and what it leads to: `Pending`, `Settled` or
`Rejected`. The HTTP layer only loads the facts under lock and applies the outcome. A replayed
approval is `409`, a vote from a non-party is `403`.

**Money.** `Amount(i64)` in minor units with a per-asset `scale` of at most 9, checked
arithmetic, no floats anywhere. Amounts enter and leave the API as decimal strings.

**Auth.** One random 256-bit key per account, shown once, stored as SHA-256. The key is a
bearer token, so transport security is TLS's job, not the application's.

**Deviations from the proposal**, each chosen to fit the time box:

* `i64` instead of `i128` (PostgreSQL `BIGINT`), and a state enum instead of a typestate: the
  state machine is four checks, a typestate would have fought the repository layer.
* A single crate with an isolated `domain` module instead of a separate domain crate.
* Not done: `Idempotency-Key`, property tests, OpenAPI. The concurrency tests are plain
  integration tests, not a dedicated stress harness.
* Demo step 5: Alice re-voting on her own offer is a replay, so it is `409`, consistent with
  step 8. A third account voting gets the `403`.

## Tests

```sh
docker compose up -d db
cargo test
```

`#[sqlx::test]` gives every test its own database, migrated from `./migrations`, and drops it
afterwards; `DATABASE_URL` comes from `.env`. Each integration test starts the real router on a
random port and talks to it over HTTP.

`tests/transactions.rs` covers the eleven demo steps plus two races: five concurrent approvals
of overlapping offers and five concurrent transfers that together overspend. Exactly one wins
and `SUM(entries)` per asset stays zero (I1, I2).

The sqlx test harness shares twenty PostgreSQL connections between all running tests;
`.cargo/config.toml` caps test threads at four so the suites stay under that limit.

## Layout

```
migrations/      schema
src/domain.rs    pure rules: Amount, legs, required approvals, the vote state machine
src/db.rs        every SQL query, compile-time checked (.sqlx/ holds the offline cache)
src/http.rs      router, AppState, the X-Account-Key extractor
src/http/        handlers: assets, accounts, transactions
src/error.rs     one error enum → HTTP status + problem+json
src/bin/server   CLI and start-up
tests/           integration tests against the real router and database
```
