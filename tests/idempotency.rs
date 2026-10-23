mod common;

use common::*;
use reqwest::Method;
use serde_json::{json, Value};
use sqlx::PgPool;
use treasury::utils::fingerprint;
use uuid::Uuid;

#[sqlx::test]
async fn replayed_transfer_runs_once(pool: PgPool) {
    let app = spawn_app(pool).await;
    let (alice, bob) = alice_and_bob(&app).await;
    let body = legs([leg(&alice.id, &bob.id, "USD", "100")]);

    let first = app
        .as_account(&alice.key, Method::POST, "/transactions")
        .header("idempotency-key", "k1")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), 201);
    let first: Value = first.json().await.unwrap();

    let second = app
        .as_account(&alice.key, Method::POST, "/transactions")
        .header("idempotency-key", "k1")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), 201);
    assert_eq!(second.headers()["content-type"], "application/json");
    let second: Value = second.json().await.unwrap();
    assert_eq!(second, first);

    let alice_usd = balance(&app.balances(&alice.key, &alice.id).await, "USD").clone();
    assert_eq!(alice_usd["available"], "900.00");
}

#[sqlx::test]
async fn same_key_with_a_different_request_is_rejected(pool: PgPool) {
    let app = spawn_app(pool).await;
    let (alice, bob) = alice_and_bob(&app).await;

    app.as_account(&alice.key, Method::POST, "/transactions")
        .header("idempotency-key", "k1")
        .json(&legs([leg(&alice.id, &bob.id, "USD", "100")]))
        .send()
        .await
        .unwrap();
    let response = app
        .as_account(&alice.key, Method::POST, "/transactions")
        .header("idempotency-key", "k1")
        .json(&legs([leg(&alice.id, &bob.id, "USD", "200")]))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 422);
    let problem: Value = response.json().await.unwrap();
    assert_eq!(
        problem["detail"],
        "idempotency key was used with a different request"
    );

    let alice_usd = balance(&app.balances(&alice.key, &alice.id).await, "USD").clone();
    assert_eq!(alice_usd["available"], "900.00");
}

#[sqlx::test]
async fn keys_are_scoped_to_the_account(pool: PgPool) {
    let app = spawn_app(pool).await;
    let (alice, bob) = alice_and_bob(&app).await;

    let from_alice: Value = app
        .as_account(&alice.key, Method::POST, "/transactions")
        .header("idempotency-key", "k1")
        .json(&legs([leg(&alice.id, &bob.id, "USD", "100")]))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let from_bob = app
        .as_account(&bob.key, Method::POST, "/transactions")
        .header("idempotency-key", "k1")
        .json(&legs([leg(&bob.id, &alice.id, "XAU", "1")]))
        .send()
        .await
        .unwrap();
    assert_eq!(from_bob.status(), 201);
    let from_bob: Value = from_bob.json().await.unwrap();
    assert_ne!(from_bob["id"], from_alice["id"]);
}

#[sqlx::test]
async fn failed_requests_are_not_remembered(pool: PgPool) {
    let app = spawn_app(pool).await;
    let (alice, bob) = alice_and_bob(&app).await;
    let body = legs([leg(&alice.id, &bob.id, "USD", "2000")]);

    let response = app
        .as_account(&alice.key, Method::POST, "/transactions")
        .header("idempotency-key", "k1")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 422);

    app.deposit(&alice.id, "USD", "1000").await;
    let response = app
        .as_account(&alice.key, Method::POST, "/transactions")
        .header("idempotency-key", "k1")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
}

#[sqlx::test]
async fn replayed_vote_is_not_a_conflict(pool: PgPool) {
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

    for _ in 0..2 {
        let response = app
            .as_account(
                &bob.key,
                Method::POST,
                &format!("/transactions/{id}/approve"),
            )
            .header("idempotency-key", "approve-once")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["state"], "settled");
    }
    assert_eq!(app.act(&bob.key, id, "approve").await.status(), 409);

    let bob_xau = balance(&app.balances(&bob.key, &bob.id).await, "XAU").clone();
    assert_eq!(bob_xau["available"], "8.500");
}

#[sqlx::test]
async fn replayed_creations_run_once(pool: PgPool) {
    let app = spawn_app(pool).await;

    let mut assets = Vec::new();
    let mut accounts = Vec::new();
    for _ in 0..2 {
        let asset = app
            .as_account(&app.operator_key, Method::POST, "/assets")
            .header("idempotency-key", "usd")
            .json(&json!({ "code": "USD", "name": "US Dollar", "scale": 2 }))
            .send()
            .await
            .unwrap();
        assert_eq!(asset.status(), 201);
        assets.push(asset.json::<Value>().await.unwrap());

        let account = app
            .as_account(&app.operator_key, Method::POST, "/accounts")
            .header("idempotency-key", "alice")
            .json(&json!({ "name": "alice" }))
            .send()
            .await
            .unwrap();
        assert_eq!(account.status(), 201);
        accounts.push(account.json::<Value>().await.unwrap());
    }
    assert_eq!(assets[0], assets[1]);
    assert_eq!(accounts[0], accounts[1]);

    let listed: Value = app.get_assets().await.json().await.unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 1);
}

#[sqlx::test]
async fn fresh_claim_is_in_progress(pool: PgPool) {
    let app = spawn_app(pool.clone()).await;
    let (alice, bob) = alice_and_bob(&app).await;
    let body = legs([leg(&alice.id, &bob.id, "USD", "100")]);
    claim(&pool, &alice.id, "k1", &body, "0 seconds").await;

    let response = app
        .as_account(&alice.key, Method::POST, "/transactions")
        .header("idempotency-key", "k1")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 409);
    let problem: Value = response.json().await.unwrap();
    assert_eq!(
        problem["detail"],
        "a request with this idempotency key is in progress"
    );

    let alice_usd = balance(&app.balances(&alice.key, &alice.id).await, "USD").clone();
    assert_eq!(alice_usd["available"], "1000.00");
}

#[sqlx::test]
async fn stale_claim_means_the_operation_never_ran(pool: PgPool) {
    let app = spawn_app(pool.clone()).await;
    let (alice, bob) = alice_and_bob(&app).await;
    let body = legs([leg(&alice.id, &bob.id, "USD", "100")]);
    claim(&pool, &alice.id, "k1", &body, "2 minutes").await;

    let mut responses = Vec::new();
    for _ in 0..2 {
        let response = app
            .as_account(&alice.key, Method::POST, "/transactions")
            .header("idempotency-key", "k1")
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 201);
        responses.push(response.json::<Value>().await.unwrap());
    }
    assert_eq!(responses[0], responses[1]);

    let alice_usd = balance(&app.balances(&alice.key, &alice.id).await, "USD").clone();
    assert_eq!(alice_usd["available"], "900.00");
}

async fn claim(pool: &PgPool, account: &str, key: &str, body: &Value, age: &str) {
    let print = fingerprint("POST", "/transactions", &serde_json::to_vec(body).unwrap());
    sqlx::query(
        "INSERT INTO idempotency_keys (account_id, key, fingerprint, claim, created_at)
         VALUES ($1, $2, $3, $4, now() - $5::interval)",
    )
    .bind(Uuid::parse_str(account).unwrap())
    .bind(key)
    .bind(print)
    .bind(Uuid::new_v4())
    .bind(age)
    .execute(pool)
    .await
    .unwrap();
}
