mod common;

use common::*;
use reqwest::Method;
use serde_json::{json, Value};
use sqlx::PgPool;
use treasury::welcome;

#[sqlx::test]
async fn create_account_as_operator(pool: PgPool) {
    let app = spawn_app(pool).await;
    let response = app
        .as_account(&app.operator_key, Method::POST, "/accounts")
        .json(&json!({ "name": "alice" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["name"], "alice");
    assert_eq!(body["key"].as_str().unwrap().len(), 64);
    assert!(uuid::Uuid::parse_str(body["id"].as_str().unwrap()).is_ok());
}

#[sqlx::test]
async fn new_key_authenticates(pool: PgPool) {
    let app = spawn_app(pool).await;
    let alice = app.create_account("alice").await;
    let response = app
        .as_account(&alice.key, Method::GET, "/assets")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
}

#[sqlx::test]
async fn anyone_registers_with_a_unique_name(pool: PgPool) {
    let app = spawn_app(pool).await;

    let response = app.register(" Carol ").await;
    assert_eq!(response.status(), 201);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["name"], "Carol");
    let key = body["key"].as_str().unwrap();
    let response = app
        .as_account(key, Method::GET, "/assets")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);

    let response = app.register("carol").await;
    assert_eq!(response.status(), 409);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["detail"], "account name already exists");
}

#[sqlx::test]
async fn names_are_validated(pool: PgPool) {
    let app = spawn_app(pool).await;
    let long = "x".repeat(33);
    let cases = [
        ("", "must not be empty"),
        ("   ", "must not be empty"),
        ("@issuer", "must not start with @"),
        (long.as_str(), "must be at most 32 characters"),
        ("a\nb", "must not contain control characters"),
    ];
    for (name, detail) in cases {
        let response = app.register(name).await;
        assert_eq!(response.status(), 422, "{name:?}");
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["detail"], format!("invalid name: {detail}"));
    }
}

#[sqlx::test]
async fn accounts_are_listed_to_any_account(pool: PgPool) {
    let app = spawn_app(pool).await;
    let bob = app.create_account("bob").await;
    let alice = app.create_account("Alice").await;

    let response = app
        .as_account(&bob.key, Method::GET, "/accounts")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    let names: Vec<&str> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|account| account["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Alice", "bob"]);
    assert_eq!(body[0]["id"], alice.id);
    assert!(body[0].get("key").is_none());

    let response = app.request(Method::GET, "/accounts").send().await.unwrap();
    assert_eq!(response.status(), 401);
}

#[sqlx::test]
async fn welcome_grant_funds_new_accounts(pool: PgPool) {
    let grant = welcome::parse("USD=1000,XAU=10").unwrap();
    let app = spawn_app_with(pool, grant).await;
    assert_eq!(app.create_asset("USD", 2).await.status(), 201);
    assert_eq!(app.create_asset("XAU", 3).await.status(), 201);

    let response = app.register("carol").await;
    assert_eq!(response.status(), 201);
    let carol = party(response).await;

    let balances = app.balances(&carol.key, &carol.id).await;
    assert_eq!(balance(&balances, "USD")["available"], "1000.00");
    assert_eq!(balance(&balances, "XAU")["available"], "10.000");

    let entries = app.entries(&carol.key, &carol.id).await;
    assert_eq!(entries.as_array().unwrap().len(), 2);
    let id = entries[0]["proposal_id"].as_str().unwrap();
    let proposal: Value = app.proposal(&carol.key, id).await.json().await.unwrap();
    assert_eq!(proposal["state"], "settled");
    assert_eq!(proposal["memo"], "welcome");
    assert_eq!(proposal["proposer"], "@issuer");

    let issuer = app.balances(&app.operator_key, "@issuer").await;
    assert_eq!(balance(&issuer, "USD")["posted"], "-1000.00");
    assert_eq!(balance(&issuer, "XAU")["posted"], "-10.000");
}

#[sqlx::test]
async fn welcome_grant_needs_its_assets(pool: PgPool) {
    let app = spawn_app_with(pool, welcome::parse("USD=1000").unwrap()).await;

    assert_eq!(app.register("carol").await.status(), 500);

    assert_eq!(app.create_asset("USD", 2).await.status(), 201);
    assert_eq!(app.register("carol").await.status(), 201);
}

#[sqlx::test]
async fn only_issuer_creates_assets(pool: PgPool) {
    let app = spawn_app(pool).await;
    let alice = app.create_account("alice").await;

    let response = app
        .as_account(&alice.key, Method::POST, "/assets")
        .json(&json!({ "code": "USD", "name": "Dollar", "scale": 2 }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["detail"], "access denied");
}

#[sqlx::test]
async fn balances_are_private(pool: PgPool) {
    let app = spawn_app(pool).await;
    let alice = app.create_account("alice").await;
    let bob = app.create_account("bob").await;

    let response = app
        .as_account(
            &alice.key,
            Method::GET,
            &format!("/accounts/{}/balances", bob.id),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);

    assert_eq!(app.balances(&alice.key, &alice.id).await, json!([]));
    assert_eq!(app.balances(&app.operator_key, &bob.id).await, json!([]));
}

#[sqlx::test]
async fn account_ref_must_be_uuid_or_issuer(pool: PgPool) {
    let app = spawn_app(pool).await;
    let response = app
        .as_account(&app.operator_key, Method::GET, "/accounts/alice/balances")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 422);

    assert_eq!(app.balances(&app.operator_key, "@issuer").await, json!([]));
}
