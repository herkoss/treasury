# Capstone project proposal — Treasury

**Author:** Dmytro Herko
**Working title:** `treasury` (an asset ledger with consent-based settlement)
**Timeframe:** 3 days for the core, one optional weekend pass for the extras
**License / visibility:** open source, public repository

---

## 1. Main objective

Build an HTTP service that keeps an **authoritative, auditable record of who owns which assets**, and
that moves those assets between accounts **only when every debited party has agreed**.

The one sentence a mentor should remember:

> Alice does not *execute* a swap — she *offers* one. Her funds are reserved the moment she offers;
> Bob's funds move only when Bob approves; and the balances change only at settlement, all legs at once
> or none at all.

Everything else in the project exists to protect these invariants:

| # | Invariant |
|---|-----------|
| I1 | **Conservation.** For every asset, the sum of all journal entries across all accounts is exactly zero. Assets are never created or destroyed outside an explicit issue/redeem against the system account. |
| I2 | **No overspend.** An account's *available* balance (`posted − on_hold`) never goes negative. |
| I3 | **Append-only history.** A committed entry is never updated or deleted. Mistakes are fixed with a compensating transaction, not with `UPDATE`. |
| I4 | **Consent.** No account is ever debited without a recorded approval from that account's owner. Being named as a debtor in someone else's proposal is inert until approved. |
| I5 | **Exactly once.** A transaction moves balances exactly once — a replayed approval is a no-op, never a double spend. |
| I6 | **Holds are released exactly once** — by settlement, by rejection, by cancellation, or by TTL expiry. |

I4 is the invariant that shapes the whole API, and it is worth stating why it exists: without it,
"transfer 1 USD from me to you, and 100 XAU from you to me" is a valid request, and the ledger becomes
a robbery tool. Consent is not a feature bolted on top of transfers — it is the reason the two-phase
flow exists at all.

## 2. Why this is not "yet another CRUD REST API"

CRUD over assets and accounts is the boring 20% of the project and takes a few hours. The interesting
80% is the settlement protocol and proving it holds under concurrency.

**The protocol is two-phase commit, with account owners as the participants:**

| 2PC | Here |
|-----|------|
| coordinator | the service |
| participants | the accounts debited by the proposal |
| *prepare* | a **hold** is placed on the approver's balance |
| *vote yes* | `POST /transactions/{id}/approve` |
| *vote no* | `POST /transactions/{id}/reject` |
| *global commit* | the **last** required approval settles the transaction — holds become journal entries |
| *global abort* | any rejection, the proposer's cancellation, or TTL expiry |

This is the classic shape, but it is deliberately **not** XA: there is one database, no distributed
resource managers, no external transaction coordinator. "Commit" is a domain protocol between *people*,
not between servers — which is exactly what makes it implementable and testable in three days.

The rest of the Rust-specific substance:

* **Making illegal states unrepresentable** — a `Transaction` value cannot be constructed unbalanced;
  the lifecycle is a typestate (`Proposal<Pending>` / `Proposal<Settled>`) so "settle a settled
  transaction" is a compile error inside the domain crate and a `409 Conflict` at the edge.
* **Money without floats** — `Amount(i128)` in minor units with a per-asset scale, checked arithmetic
  everywhere, `TryFrom` at the API boundary, no `f64` anywhere in the codebase.
* **Concurrency that is actually tested** — rows locked in a deterministic order (`ORDER BY account_id`)
  so deadlocks are impossible by construction, plus a stress test that fires N concurrent approvals of
  overlapping proposals and asserts I1–I6 afterwards. The interesting race is real: two proposals both
  approved at the same instant, together exceeding the balance — exactly one must win.
* **Property-based testing** — `proptest` generates random sequences of propose/approve/reject/expire
  and checks the invariants after every sequence, not just on the happy path.
* **Idempotency** — every mutating request carries an `Idempotency-Key`; a replay returns the original
  response instead of creating a second proposal or a second approval.

## 3. Domain model

| Entity | Purpose |
|--------|---------|
| **Asset** | A kind of thing being tracked: `code`, `name`, `scale` (decimals), `status`. |
| **Account** | An owner of balances. Created with an **account key** returned exactly once — the bearer of that key acts as the owner. One system account (`@issuer`) is the counterparty for deposits and withdrawals, authorised by the operator key. |
| **Proposal** (transaction) | A set of legs that nets to zero per asset, a proposer, a required-approval set, and a state. |
| **Approval** | `(proposal_id, account_id, decision, decided_at)` — the recorded vote. Append-only. |
| **Hold** | An amount locked on an account for a pending proposal, with an expiry. |
| **Entry** | An immutable, signed journal line: `(proposal_id, account_id, asset_id, amount)`, written only at settlement. |

**Required approvals** are derived, not supplied: they are the distinct set of accounts appearing on the
debit side of any leg. The proposer's own accounts are approved implicitly at creation — offering *is*
consenting. So:

* a plain **transfer** from my account debits only me → the set is `{me}` → it settles in one call,
  exactly as before;
* a **swap** debits both sides → the set is `{alice, bob}` → alice's approval is implicit, bob's is not;
* an **N-party barter** works with no extra code — three parties, three approvals, settles on the third.

**Holds follow approvals.** Alice's funds are locked when she proposes; Bob's are locked when he
approves. When the last approval arrives, every hold is converted into journal entries inside a single
database transaction — so a hold is never observable in a settled state.

Balances are **derived** from the journal (with a materialised per-`(account, asset)` row for speed),
never written directly.

## 4. API surface (frozen on day 1)

Mutating requests carry `X-Account-Key` (the acting account) and `Idempotency-Key`.

```
POST   /assets                      create
GET    /assets                      list
GET    /assets/{code}               read
PATCH  /assets/{code}               update mutable metadata
DELETE /assets/{code}               archive — 409 if the asset still has non-zero balances

POST   /accounts                    create — returns the account key once
GET    /accounts/{id}/balances      posted / on_hold / available, per asset
GET    /accounts/{id}/entries       paginated journal

POST   /transactions                propose — holds the proposer's legs, returns required approvals
GET    /accounts/{id}/inbox         proposals awaiting MY approval
GET    /accounts/{id}/outbox        proposals I made that are still pending
GET    /transactions/{id}           read, including every approval and its timestamp
POST   /transactions/{id}/approve   my vote — the last one settles the whole thing
POST   /transactions/{id}/reject    my refusal — every hold is released
POST   /transactions/{id}/cancel    the proposer withdraws the offer
```

One request shape still covers transfer, exchange, deposit and withdrawal — the difference is only the
legs:

```jsonc
{
  "memo": "100 USD for 1.5 XAU",
  "expires_in_secs": 900,
  "legs": [
    { "from": "acc_alice", "to": "acc_bob",   "asset": "USD", "amount": "100.00" },
    { "from": "acc_bob",   "to": "acc_alice", "asset": "XAU", "amount": "1.5"    }
  ]
}
```

* one leg, debiting only the proposer → **transfer**, settles immediately
* legs debiting someone else → **offer**, lands in that account's inbox
* `"from": "@issuer"` → **deposit**, `"to": "@issuer"` → **withdrawal**

The response carries the state machine explicitly:

```jsonc
{
  "id": "txn_01J…",
  "state": "pending",
  "expires_at": "2026-09-10T12:15:00Z",
  "required_approvals": [
    { "account": "acc_alice", "decision": "approved", "at": "2026-09-10T12:00:00Z" },
    { "account": "acc_bob",   "decision": "pending",  "at": null }
  ]
}
```

## 5. Out of scope — what the project will **not** do

Explicitly excluded so that the core fits into three days:

* **No negotiation.** Bob can approve or reject, not counter-offer. A different price means a new
  proposal. (Counter-offers are a stretch goal, not part of the objective.)
* **No marketplace.** Offers are addressed to specific accounts and appear only in their inbox. There is
  no public order book, no discovery, no browsing of other people's offers.
* **No pricing.** The rate arrives in the request. No matching engine, no FX rate discovery — the
  service *records* an agreed swap, it does not *price* it and does not judge whether it is fair.
* **No real money.** No bank, PSP, blockchain or custody integrations. No settlement outside the service.
* **No distributed transactions.** The protocol is the domain-level 2PC of §2, inside one PostgreSQL
  database. No XA, no sagas across services, no Kafka/outbox.
* **No user/identity system.** One opaque key per account, one operator key for `@issuer`. No users,
  roles, tenants, OAuth, JWT, key rotation or delegation.
* **No UI.** HTTP plus a generated OpenAPI document is the whole interface.
* **No reporting or analytics.** No statements, no P&L, no CSV/PDF export.
* **No notifications.** The inbox is polled; no email, no webhooks, no websockets.
* **No horizontal scaling story.** Single instance, single database. No sharding or CQRS read models.
* **No hard deletes.** Assets and accounts are archived; history is never removed.

## 6. Stack

`axum` + `tokio` · `sqlx` + PostgreSQL (compile-time checked queries, migrations) · `serde` ·
`thiserror` (typed errors → `application/problem+json`) · `uuid` · `tracing` + `tracing-subscriber` ·
`utoipa` (OpenAPI) · `config` + `clap` · `proptest` · `reqwest` for integration tests · `docker compose`
for a one-command local run.

The domain lives in its own crate with **no I/O dependencies**, so the approval state machine and the
invariants can be property-tested without a database.

## 7. Plan

**Day 1 — domain**
Domain crate: assets, accounts, `Amount`, balanced-proposal builder, required-approval derivation, the
approve/reject/cancel/expire state machine, balance projection. Unit tests + first property tests.
SQL schema and migrations.

**Day 2 — service**
axum routes, account keys and the "who is acting" check, DTOs, error mapping, idempotency, holds placed
per approval, settlement in one database transaction, inbox/outbox, the TTL expiry task, OpenAPI.
Integration tests against a real PostgreSQL.

**Day 3 — hardening and delivery**
Locking strategy and the concurrent stress test, the consent test suite (§8 steps 4–6), README with a
`curl` walkthrough, `docker compose up`, CI (fmt, clippy `-D warnings`, tests).

**Optional weekend pass — extras only.**
The three days above produce a complete, demonstrable service. Anything from §10 is done afterwards and
is not part of what I am asking to have approved.

## 8. Definition of done — the demo script

A reviewer clones the repo, runs `docker compose up`, and follows the README:

1. create two assets and two accounts, keeping both account keys;
2. deposit `1000 USD` to Alice and `10 XAU` to Bob from `@issuer`;
3. Alice transfers `100 USD` to Bob → single required approval, settles at once, balances move;
4. **Alice proposes "1 USD for 100 XAU"** → `202`, nothing moves, `required_approvals` shows Bob pending;
5. Alice tries to approve on Bob's behalf with her own key → `403`, still nothing moves;
6. Bob reads `GET /accounts/acc_bob/inbox`, sees the offer, `POST .../reject` → every hold released,
   Bob's XAU untouched;
7. Alice proposes a fair `100 USD ↔ 1.5 XAU`, Bob approves → both legs move, or neither;
8. approve the same proposal again → `409`, balances unchanged;
9. Alice proposes with `expires_in_secs: 1` and waits → state `expired`, her hold released;
10. a second proposal spending funds already held → `422 insufficient available balance`;
11. `GET /accounts/{id}/entries` → the full, unchanged audit trail, every settlement traceable to the
    approvals that authorised it.

Plus: `cargo test` green, including the property tests and the concurrency stress test.

## 9. Risks and mitigations

| Risk | Mitigation |
|------|------------|
| Consent flow pushes the core past three days | The CRUD half is trivial; the state machine is one enum and one derived set. If day 2 slips, OpenAPI polish and asset archiving are dropped first — never the consent checks. |
| A pending offer is a free option: Bob waits to see if the price moves | TTL is mandatory, defaults to 15 minutes and is capped server-side; Alice's exposure is visible to her as a hold and she can cancel any time before the last approval. Stated as a deliberate design limit, not solved by pricing. |
| Offer spam — anyone can fill my inbox with proposals | Inbox entries are inert and cost the recipient nothing; a per-account allowlist is listed as a stretch goal, not core. |
| PostgreSQL isolation subtleties eating a day | Pessimistic `SELECT … FOR UPDATE` in a fixed key order, `READ COMMITTED`. Serializable + retry only if time remains. |
| Typestate turning into type astronautics | Typestate is used for exactly one thing — the proposal lifecycle. If it fights the repository layer, it is dropped in favour of a checked enum. |

## 10. Stretch goals (weekend pass, explicitly outside the approved objective)

* **Counter-offers** — Bob responds with an amended proposal that supersedes the original.
* **Per-account allowlist** — only accepts proposals from named counterparties.
* Compensating reversal endpoint (`POST /transactions/{id}/reverse`).
* `GET /accounts/{id}/balances?at=<timestamp>` — balance at a point in time, replayed from the journal.
* Webhooks on inbox changes, so the counterparty does not have to poll.
* A tiny CLI client over the same API.
