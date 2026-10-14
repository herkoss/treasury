mod common;

use std::time::Duration;

use common::*;
use reqwest::Method;
use serde_json::{json, Value};
use sqlx::PgPool;

#[sqlx::test]
async fn deposit_from_issuer_settles_at_once(pool: PgPool) {
    let app = spawn_app(pool).await;
    app.create_asset("USD", 2).await;
    let alice = app.create_account("alice").await;

    let proposal = app.deposit(&alice.id, "USD", "1000").await;
    assert_eq!(proposal["state"], "settled");
    assert_eq!(proposal["legs"][0]["from"], "@issuer");
    assert_eq!(proposal["legs"][0]["amount"], "1000.00");
    assert_eq!(approval(&proposal, "@issuer")["decision"], "approved");
    assert!(proposal["closed_at"].is_string());

    let alice_usd = balance(&app.balances(&alice.key, &alice.id).await, "USD").clone();
    assert_eq!(alice_usd["posted"], "1000.00");
    assert_eq!(alice_usd["on_hold"], "0.00");
    assert_eq!(alice_usd["available"], "1000.00");

    let issuer_usd = balance(&app.balances(&app.operator_key, "@issuer").await, "USD").clone();
    assert_eq!(issuer_usd["posted"], "-1000.00");
}

#[sqlx::test]
async fn transfer_settles_with_a_single_approval(pool: PgPool) {
    let app = spawn_app(pool).await;
    let (alice, bob) = alice_and_bob(&app).await;

    let response = app
        .propose(&alice.key, legs([leg(&alice.id, &bob.id, "USD", "100")]))
        .await;
    assert_eq!(response.status(), 201);
    let proposal: Value = response.json().await.unwrap();
    assert_eq!(proposal["state"], "settled");
    assert_eq!(proposal["required_approvals"].as_array().unwrap().len(), 1);

    let alice_usd = balance(&app.balances(&alice.key, &alice.id).await, "USD").clone();
    assert_eq!(alice_usd["available"], "900.00");
    let bob_usd = balance(&app.balances(&bob.key, &bob.id).await, "USD").clone();
    assert_eq!(bob_usd["available"], "100.00");
}

#[sqlx::test]
async fn offer_holds_only_the_proposer(pool: PgPool) {
    let app = spawn_app(pool).await;
    let (alice, bob) = alice_and_bob(&app).await;

    let response = app
        .propose(
            &alice.key,
            json!({
                "memo": "1 USD for 100 XAU",
                "legs": [leg(&alice.id, &bob.id, "USD", "1"), leg(&bob.id, &alice.id, "XAU", "100")]
            }),
        )
        .await;
    assert_eq!(response.status(), 202);
    let proposal: Value = response.json().await.unwrap();
    assert_eq!(proposal["state"], "pending");
    assert_eq!(proposal["memo"], "1 USD for 100 XAU");
    assert_eq!(approval(&proposal, &alice.id)["decision"], "approved");
    assert_eq!(approval(&proposal, &bob.id)["decision"], "pending");
    assert!(approval(&proposal, &bob.id)["at"].is_null());

    let alice_usd = balance(&app.balances(&alice.key, &alice.id).await, "USD").clone();
    assert_eq!(alice_usd["posted"], "1000.00");
    assert_eq!(alice_usd["on_hold"], "1.00");
    assert_eq!(alice_usd["available"], "999.00");
    let bob_xau = balance(&app.balances(&bob.key, &bob.id).await, "XAU").clone();
    assert_eq!(bob_xau["on_hold"], "0.000");
    assert_eq!(bob_xau["available"], "10.000");

    let inbox = app.inbox(&bob.key, &bob.id).await;
    assert_eq!(inbox[0]["id"], proposal["id"]);
    let outbox = app.outbox(&alice.key, &alice.id).await;
    assert_eq!(outbox[0]["id"], proposal["id"]);
    assert_eq!(app.inbox(&alice.key, &alice.id).await, json!([]));
}

#[sqlx::test]
async fn only_a_debited_party_can_vote(pool: PgPool) {
    let app = spawn_app(pool).await;
    let (alice, bob) = alice_and_bob(&app).await;
    let carol = app.create_account("carol").await;
    let proposal: Value = app
        .propose(
            &alice.key,
            legs([
                leg(&alice.id, &bob.id, "USD", "1"),
                leg(&bob.id, &alice.id, "XAU", "100"),
            ]),
        )
        .await
        .json()
        .await
        .unwrap();
    let id = proposal["id"].as_str().unwrap();

    assert_eq!(app.act(&carol.key, id, "approve").await.status(), 403);
    assert_eq!(
        app.act(&app.operator_key, id, "approve").await.status(),
        403
    );
    assert_eq!(app.proposal(&carol.key, id).await.status(), 403);

    let response = app.act(&alice.key, id, "approve").await;
    assert_eq!(response.status(), 409);
    let body: Value = response.json().await.unwrap();
    assert_eq!(
        body["detail"],
        "invalid proposal this account has already decided"
    );

    let bob_xau = balance(&app.balances(&bob.key, &bob.id).await, "XAU").clone();
    assert_eq!(bob_xau["available"], "10.000");
}

#[sqlx::test]
async fn rejection_releases_every_hold(pool: PgPool) {
    let app = spawn_app(pool).await;
    let (alice, bob) = alice_and_bob(&app).await;
    let proposal: Value = app
        .propose(
            &alice.key,
            legs([
                leg(&alice.id, &bob.id, "USD", "1"),
                leg(&bob.id, &alice.id, "XAU", "100"),
            ]),
        )
        .await
        .json()
        .await
        .unwrap();
    let id = proposal["id"].as_str().unwrap();

    let response = app.act(&bob.key, id, "reject").await;
    assert_eq!(response.status(), 200);
    let proposal: Value = response.json().await.unwrap();
    assert_eq!(proposal["state"], "rejected");
    assert_eq!(approval(&proposal, &bob.id)["decision"], "rejected");

    let alice_usd = balance(&app.balances(&alice.key, &alice.id).await, "USD").clone();
    assert_eq!(alice_usd["on_hold"], "0.00");
    assert_eq!(alice_usd["available"], "1000.00");
    let bob_xau = balance(&app.balances(&bob.key, &bob.id).await, "XAU").clone();
    assert_eq!(bob_xau["available"], "10.000");
    assert_eq!(app.inbox(&bob.key, &bob.id).await, json!([]));
    assert_eq!(app.act(&bob.key, id, "approve").await.status(), 409);
}

#[sqlx::test]
async fn swap_settles_both_legs_on_the_last_approval(pool: PgPool) {
    let app = spawn_app(pool).await;
    let (alice, bob) = alice_and_bob(&app).await;
    let proposal: Value = app
        .propose(
            &alice.key,
            legs([
                leg(&alice.id, &bob.id, "USD", "100"),
                leg(&bob.id, &alice.id, "XAU", "1.5"),
            ]),
        )
        .await
        .json()
        .await
        .unwrap();
    let id = proposal["id"].as_str().unwrap();

    let response = app.act(&bob.key, id, "approve").await;
    assert_eq!(response.status(), 200);
    let proposal: Value = response.json().await.unwrap();
    assert_eq!(proposal["state"], "settled");
    assert_eq!(approval(&proposal, &bob.id)["decision"], "approved");
    assert!(approval(&proposal, &bob.id)["at"].is_string());

    let alice_balances = app.balances(&alice.key, &alice.id).await;
    assert_eq!(balance(&alice_balances, "USD")["available"], "900.00");
    assert_eq!(balance(&alice_balances, "XAU")["available"], "1.500");
    let bob_balances = app.balances(&bob.key, &bob.id).await;
    assert_eq!(balance(&bob_balances, "USD")["available"], "100.00");
    assert_eq!(balance(&bob_balances, "XAU")["available"], "8.500");
    assert_eq!(balance(&bob_balances, "XAU")["on_hold"], "0.000");

    let response = app.act(&bob.key, id, "approve").await;
    assert_eq!(response.status(), 409);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["detail"], "invalid proposal proposal is settled");
    let bob_balances = app.balances(&bob.key, &bob.id).await;
    assert_eq!(balance(&bob_balances, "XAU")["available"], "8.500");
}

#[sqlx::test]
async fn proposer_can_cancel_a_pending_offer(pool: PgPool) {
    let app = spawn_app(pool).await;
    let (alice, bob) = alice_and_bob(&app).await;
    let proposal: Value = app
        .propose(
            &alice.key,
            legs([
                leg(&alice.id, &bob.id, "USD", "100"),
                leg(&bob.id, &alice.id, "XAU", "1.5"),
            ]),
        )
        .await
        .json()
        .await
        .unwrap();
    let id = proposal["id"].as_str().unwrap();

    assert_eq!(app.act(&bob.key, id, "cancel").await.status(), 403);
    let response = app.act(&alice.key, id, "cancel").await;
    assert_eq!(response.status(), 200);
    let proposal: Value = response.json().await.unwrap();
    assert_eq!(proposal["state"], "cancelled");

    let alice_usd = balance(&app.balances(&alice.key, &alice.id).await, "USD").clone();
    assert_eq!(alice_usd["available"], "1000.00");
    assert_eq!(app.act(&bob.key, id, "approve").await.status(), 409);
    assert_eq!(app.outbox(&alice.key, &alice.id).await, json!([]));
}

#[sqlx::test]
async fn expired_offer_releases_the_hold(pool: PgPool) {
    let app = spawn_app(pool).await;
    let (alice, bob) = alice_and_bob(&app).await;
    let proposal: Value = app
        .propose(
            &alice.key,
            json!({
                "expires_in_secs": 1,
                "legs": [leg(&alice.id, &bob.id, "USD", "100"), leg(&bob.id, &alice.id, "XAU", "1.5")]
            }),
        )
        .await
        .json()
        .await
        .unwrap();
    let id = proposal["id"].as_str().unwrap();
    assert_eq!(
        balance(&app.balances(&alice.key, &alice.id).await, "USD")["on_hold"],
        "100.00"
    );

    tokio::time::sleep(Duration::from_millis(1100)).await;

    let proposal: Value = app.proposal(&alice.key, id).await.json().await.unwrap();
    assert_eq!(proposal["state"], "expired");
    let alice_usd = balance(&app.balances(&alice.key, &alice.id).await, "USD").clone();
    assert_eq!(alice_usd["on_hold"], "0.00");
    assert_eq!(alice_usd["available"], "1000.00");
    assert_eq!(app.inbox(&bob.key, &bob.id).await, json!([]));

    let response = app.act(&bob.key, id, "approve").await;
    assert_eq!(response.status(), 409);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["detail"], "invalid proposal proposal is expired");
}

#[sqlx::test]
async fn sweeper_closes_expired_rows(pool: PgPool) {
    let app = spawn_app(pool.clone()).await;
    let (alice, bob) = alice_and_bob(&app).await;
    let proposal: Value = app
        .propose(
            &alice.key,
            json!({
                "expires_in_secs": 1,
                "legs": [leg(&alice.id, &bob.id, "USD", "100"), leg(&bob.id, &alice.id, "XAU", "1.5")]
            }),
        )
        .await
        .json()
        .await
        .unwrap();
    let id = uuid::Uuid::parse_str(proposal["id"].as_str().unwrap()).unwrap();

    tokio::time::sleep(Duration::from_millis(1500)).await;

    let row = sqlx::query!(
        "SELECT state, closed_at, expires_at FROM proposals WHERE id = $1",
        id
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.state, "expired");
    assert_eq!(row.closed_at, Some(row.expires_at));
}

#[sqlx::test]
async fn held_funds_cannot_be_promised_twice(pool: PgPool) {
    let app = spawn_app(pool).await;
    let (alice, bob) = alice_and_bob(&app).await;
    let offer = legs([
        leg(&alice.id, &bob.id, "USD", "600"),
        leg(&bob.id, &alice.id, "XAU", "1"),
    ]);

    assert_eq!(app.propose(&alice.key, offer.clone()).await.status(), 202);

    let response = app.propose(&alice.key, offer).await;
    assert_eq!(response.status(), 422);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["detail"], "insufficient available balance of USD");

    let response = app
        .propose(&alice.key, legs([leg(&alice.id, &bob.id, "USD", "400")]))
        .await;
    assert_eq!(response.status(), 201);
    let alice_usd = balance(&app.balances(&alice.key, &alice.id).await, "USD").clone();
    assert_eq!(alice_usd["available"], "0.00");
}

#[sqlx::test]
async fn approver_needs_available_funds_too(pool: PgPool) {
    let app = spawn_app(pool).await;
    let (alice, bob) = alice_and_bob(&app).await;
    let proposal: Value = app
        .propose(
            &alice.key,
            legs([
                leg(&alice.id, &bob.id, "USD", "1"),
                leg(&bob.id, &alice.id, "XAU", "100"),
            ]),
        )
        .await
        .json()
        .await
        .unwrap();
    let id = proposal["id"].as_str().unwrap();

    let response = app.act(&bob.key, id, "approve").await;
    assert_eq!(response.status(), 422);
    let proposal: Value = app.proposal(&bob.key, id).await.json().await.unwrap();
    assert_eq!(proposal["state"], "pending");
    assert_eq!(approval(&proposal, &bob.id)["decision"], "pending");
}

#[sqlx::test]
async fn journal_records_every_settlement(pool: PgPool) {
    let app = spawn_app(pool).await;
    let (alice, bob) = alice_and_bob(&app).await;
    let proposal: Value = app
        .propose(
            &alice.key,
            legs([
                leg(&alice.id, &bob.id, "USD", "100"),
                leg(&bob.id, &alice.id, "XAU", "1.5"),
            ]),
        )
        .await
        .json()
        .await
        .unwrap();
    let id = proposal["id"].as_str().unwrap();
    app.act(&bob.key, id, "approve").await;

    let entries = app.entries(&alice.key, &alice.id).await;
    let rows = entries.as_array().unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["asset"], "USD");
    assert_eq!(rows[0]["amount"], "1000.00");
    assert_eq!(rows[1]["asset"], "USD");
    assert_eq!(rows[1]["amount"], "-100.00");
    assert_eq!(rows[1]["proposal_id"], id);
    assert_eq!(rows[2]["asset"], "XAU");
    assert_eq!(rows[2]["amount"], "1.500");
    assert_eq!(rows[2]["proposal_id"], id);

    let response = app
        .as_account(
            &alice.key,
            Method::GET,
            &format!(
                "/accounts/{}/entries?after={}&limit=1",
                alice.id, rows[0]["id"]
            ),
        )
        .send()
        .await
        .unwrap();
    let page: Value = response.json().await.unwrap();
    assert_eq!(page.as_array().unwrap().len(), 1);
    assert_eq!(page[0]["id"], rows[1]["id"]);
}

#[sqlx::test]
async fn malformed_proposals_are_rejected(pool: PgPool) {
    let app = spawn_app(pool).await;
    let (alice, bob) = alice_and_bob(&app).await;

    let cases = [
        (legs([]), 422, "invalid legs legs cannot be empty"),
        (
            legs([leg(&alice.id, &alice.id, "USD", "1")]),
            422,
            "invalid legs sender and receiver cannot be the same",
        ),
        (
            legs([leg(&alice.id, &bob.id, "USD", "1.005")]),
            422,
            "invalid amount fractional part exceeds the asset scale",
        ),
        (
            legs([leg(&alice.id, &bob.id, "USD", "-1")]),
            422,
            "invalid amount amount must be digits with at most one dot",
        ),
        (
            legs([leg(&alice.id, &bob.id, "EUR", "1")]),
            404,
            "asset not found",
        ),
        (
            legs([leg(&alice.id, "bob", "USD", "1")]),
            422,
            "invalid account \"bob\"",
        ),
        (
            legs([leg(
                &alice.id,
                &uuid::Uuid::new_v4().to_string(),
                "USD",
                "1",
            )]),
            404,
            "account not found",
        ),
    ];
    for (body, status, detail) in cases {
        let response = app.propose(&alice.key, body).await;
        assert_eq!(response.status(), status);
        let problem: Value = response.json().await.unwrap();
        assert_eq!(problem["detail"], detail);
    }
}

#[sqlx::test]
async fn archived_asset_cannot_be_used(pool: PgPool) {
    let app = spawn_app(pool).await;
    let (alice, bob) = alice_and_bob(&app).await;

    let response = app
        .as_account(&app.operator_key, Method::DELETE, "/assets/USD")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 409);

    app.create_asset("EUR", 2).await;
    let response = app
        .as_account(&app.operator_key, Method::DELETE, "/assets/EUR")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let asset: Value = response.json().await.unwrap();
    assert!(asset["archived_at"].is_string());

    let response = app
        .propose(&alice.key, legs([leg(&alice.id, &bob.id, "EUR", "1")]))
        .await;
    assert_eq!(response.status(), 404);
}

#[sqlx::test]
async fn concurrent_approvals_settle_exactly_once(pool: PgPool) {
    let app = spawn_app(pool.clone()).await;
    let (alice, bob) = alice_and_bob(&app).await;

    let mut ids = Vec::new();
    for _ in 0..5 {
        let proposal: Value = app
            .propose(
                &alice.key,
                legs([
                    leg(&alice.id, &bob.id, "USD", "1"),
                    leg(&bob.id, &alice.id, "XAU", "6"),
                ]),
            )
            .await
            .json()
            .await
            .unwrap();
        ids.push(proposal["id"].as_str().unwrap().to_string());
    }

    let mut tasks = Vec::new();
    for id in ids {
        let request = app.as_account(
            &bob.key,
            Method::POST,
            &format!("/transactions/{id}/approve"),
        );
        tasks.push(tokio::spawn(async move {
            request.send().await.unwrap().status().as_u16()
        }));
    }
    let mut statuses = Vec::new();
    for task in tasks {
        statuses.push(task.await.unwrap());
    }
    assert_eq!(statuses.iter().filter(|s| **s == 200).count(), 1);
    assert_eq!(statuses.iter().filter(|s| **s == 422).count(), 4);

    let bob_xau = balance(&app.balances(&bob.key, &bob.id).await, "XAU").clone();
    assert_eq!(bob_xau["available"], "4.000");
    assert_conservation(&pool).await;
}

#[sqlx::test]
async fn concurrent_transfers_never_overspend(pool: PgPool) {
    let app = spawn_app(pool.clone()).await;
    let (alice, bob) = alice_and_bob(&app).await;

    let mut tasks = Vec::new();
    for _ in 0..5 {
        let request = app
            .as_account(&alice.key, Method::POST, "/transactions")
            .json(&legs([leg(&alice.id, &bob.id, "USD", "600")]));
        tasks.push(tokio::spawn(async move {
            request.send().await.unwrap().status().as_u16()
        }));
    }
    let mut statuses = Vec::new();
    for task in tasks {
        statuses.push(task.await.unwrap());
    }
    assert_eq!(statuses.iter().filter(|s| **s == 201).count(), 1);
    assert_eq!(statuses.iter().filter(|s| **s == 422).count(), 4);

    let alice_usd = balance(&app.balances(&alice.key, &alice.id).await, "USD").clone();
    assert_eq!(alice_usd["available"], "400.00");
    assert_conservation(&pool).await;
}

async fn assert_conservation(pool: &PgPool) {
    let totals = sqlx::query!(
        r#"SELECT asset_code, COALESCE(SUM(amount), 0)::BIGINT AS "total!" FROM entries GROUP BY asset_code"#
    )
    .fetch_all(pool)
    .await
    .unwrap();
    assert!(!totals.is_empty());
    for row in totals {
        assert_eq!(row.total, 0, "asset {} is not conserved", row.asset_code);
    }
}
