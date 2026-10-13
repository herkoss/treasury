# Domain model

## Entities

| Entity | What it is | Lives in |
|---|---|---|
| **Asset** | A kind of thing tracked: `USD`, `XAU`. `scale` says how many decimals an amount has. | `assets` |
| **Account** | A party that can own balances. Identified by an opaque key shown once at creation. The `@issuer` account is the counterparty for deposits and withdrawals. | `accounts` |
| **Proposal** | An offer to move assets: a set of legs, a proposer, a state, an expiry. | `proposals` |
| **Leg** | One movement inside a proposal: `from` gives `amount` of `asset` to `to`. | `legs` |
| **Approval** | One account's recorded vote on one proposal: `approved` or `rejected`. Append-only. | `approvals` |
| **Entry** | One immutable journal line: account, asset, signed amount. Written only at settlement. | `entries` |

Relationships: a proposal has 1..16 legs, each leg references two accounts and one asset.
A proposal has at most one approval per account. A settled proposal has exactly two entries per leg
(debit on `from`, credit on `to`). An account's balance is the sum of its entries.

## Derived facts (never stored)

| Fact | Definition |
|---|---|
| `required(proposal)` | distinct `from` accounts of its legs: everyone who gives something must consent |
| `debits(proposal)` | sum of amounts per `(from, asset)`: what each account must have available |
| `posted(account, asset)` | `SUM(entries.amount)` for that pair |
| `on_hold(account, asset)` | sum of pending legs where the account is `from` and has an `approved` row |
| `available` | `posted - on_hold`, never allowed below zero (except for `@issuer`) |
| effective state | `expired` if the stored state is `pending` and `expires_at` has passed |

Because holds are derived, closing a proposal (settle, reject, cancel, expiry) releases them with no extra write.

## Lifecycle of a proposal

```
                 propose
                   │
                   ▼
              ┌─────────┐  last required approval   ┌─────────┐
              │ pending │ ─────────────────────────▶ │ settled │  entries written
              └─────────┘                            └─────────┘
                │  │  │
   any reject   │  │  │  proposer cancels
                ▼  │  ▼
         ┌──────────┐ ┌───────────┐
         │ rejected │ │ cancelled │
         └──────────┘ └───────────┘
                   │
                   ▼  expires_at passes
              ┌─────────┐
              │ expired │
              └─────────┘
```

Offering is consenting: the proposer gets an `approved` row the moment the proposal is created.
A proposal whose only required account is the proposer therefore settles immediately (a plain transfer).

## Walkthrough: Alice offers 100 USD for 1.5 XAU

1. **Alice: `POST /transactions`** with two legs (alice→bob 100 USD, bob→alice 1.5 XAU).
   Server: validate legs → required = {alice, bob}, debits = {(alice, USD): 100}.
   Check Alice's available USD ≥ 100. Insert proposal (pending), legs, approval (alice, approved).
   Alice's 100 USD is now on hold. Response `202`: alice approved, bob pending.
2. **Bob: `GET /accounts/bob/inbox`** → sees the offer.
3. **Bob: `POST /transactions/{id}/approve`**. Server loads the facts: effective state, required,
   approved, proposer. Asks the domain what happens (`Settled`). Checks Bob's available XAU ≥ 1.5.
   Inserts approval (bob, approved), writes four entries (−100 USD alice, +100 USD bob,
   −1.5 XAU bob, +1.5 XAU alice), sets state `settled`. One database transaction.
   Had Bob rejected: approval (bob, rejected), state `rejected`, nothing else. Alice's hold is gone
   because on_hold only counts pending proposals.

## The domain module

The server has no in-memory state: every request starts by loading facts from the database and ends
by writing the result back. So the domain is not objects with mutable balances; it is pure functions
that answer questions about facts. The server supplies the facts, the domain decides, the server executes.

| Function | Question it answers | Used by |
|---|---|---|
| `Amount::parse` / `format` | what is "100.00" in minor units, and back | request parsing, responses |
| `validate_legs` | is this set of legs well-formed | `POST /transactions` |
| `required_approvals` | whose consent is needed | propose, approve, reject, `GET /transactions/{id}` |
| `debit` | how much must each account have available | propose, approve |
| `Proposal::vote` | given who is required and who already approved, what does this vote do | approve, reject |
| `Proposal::cancel` | may this actor withdraw the offer | cancel |

`Proposal::vote` returns an `Outcome`, which is the server's to-do list:

| Outcome | Server does |
|---|---|
| `Settled` | record the vote, write entries, close the proposal |
| `Pending` | record the vote, nothing else |
| `Rejected` | record the vote, close the proposal |

Errors from the domain map to HTTP: malformed input → 422, wrong actor → 403, wrong state → 409.
