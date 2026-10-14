mod common;

use common::*;
use reqwest::Method;
use serde_json::{json, Value};
use sqlx::PgPool;

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
async fn only_issuer_creates_accounts_and_assets(pool: PgPool) {
    let app = spawn_app(pool).await;
    let alice = app.create_account("alice").await;

    let response = app
        .as_account(&alice.key, Method::POST, "/accounts")
        .json(&json!({ "name": "mallory" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);

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
