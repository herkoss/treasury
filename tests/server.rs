mod common;

use common::*;
use reqwest::Method;
use sqlx::PgPool;

#[sqlx::test]
async fn health_check(pool: PgPool) {
    let app = spawn_app(pool).await;

    let response = app.client.get(app.url("/health")).send().await.unwrap();

    assert_eq!(response.status(), 200);
}

#[sqlx::test]
async fn create_asset_as_operator(pool: PgPool) {
    let app = spawn_app(pool).await;
    let response = app.create_asset("BTC", 8).await;
    assert_eq!(response.status(), 201);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["code"], "BTC");
    assert_eq!(body["scale"], 8);
    assert!(body["archived_at"].is_null());
}

#[sqlx::test]
async fn create_asset_without_key(pool: PgPool) {
    let app = spawn_app(pool).await;
    let response = app
        .request(Method::POST, "/assets")
        .json(&serde_json::json!({ "code": "BTC", "name": "Bitcoin", "scale": 8 }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
    assert_eq!(
        response.headers()["content-type"],
        "application/problem+json"
    );
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["status"], 401);
    assert_eq!(body["detail"], "missing or invalid token");
}

#[sqlx::test]
async fn create_asset_twice(pool: PgPool) {
    let app = spawn_app(pool).await;
    app.create_asset("BTC", 8).await;
    let response = app.create_asset("BTC", 8).await;
    assert_eq!(response.status(), 409);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["detail"], "asset already exists");
}

#[sqlx::test]
async fn get_assets(pool: PgPool) {
    let app = spawn_app(pool).await;
    let assets_resp = app.create_asset("BTC", 8).await;
    let asset: serde_json::Value = assets_resp.json().await.unwrap();

    let response = app.get_assets().await;
    let assets: serde_json::Value = response.json().await.unwrap();

    assert_eq!(serde_json::json!([asset]), assets);
}

#[sqlx::test]
async fn get_assets_sorted_by_code(pool: PgPool) {
    let app = spawn_app(pool).await;
    let eth: serde_json::Value = app.create_asset("ETH", 6).await.json().await.unwrap();
    let btc: serde_json::Value = app.create_asset("BTC", 8).await.json().await.unwrap();

    let assets: serde_json::Value = app.get_assets().await.json().await.unwrap();

    assert_eq!(assets, serde_json::json!([btc, eth]));
}

#[sqlx::test]
async fn get_assets_without_key(pool: PgPool) {
    let app = spawn_app(pool).await;
    let response = app.request(Method::GET, "/assets").send().await.unwrap();
    assert_eq!(response.status(), 401);
}

#[sqlx::test]
async fn get_asset_by_code(pool: PgPool) {
    let app = spawn_app(pool).await;
    let created: serde_json::Value = app.create_asset("BTC", 8).await.json().await.unwrap();

    let response = app.get_asset("BTC").await;
    assert_eq!(response.status(), 200);
    let asset: serde_json::Value = response.json().await.unwrap();
    assert_eq!(asset, created);
}

#[sqlx::test]
async fn get_unknown_asset(pool: PgPool) {
    let app = spawn_app(pool).await;
    let response = app.get_asset("BTC").await;
    assert_eq!(response.status(), 404);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["detail"], "asset not found");
}

#[sqlx::test]
async fn openapi_is_served(pool: PgPool) {
    let app = spawn_app(pool).await;

    let response = app
        .client
        .get(app.url("/openapi.json"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let spec: serde_json::Value = response.json().await.unwrap();
    assert!(spec["paths"]["/transactions"]["post"].is_object());
    assert!(spec["paths"]["/accounts/{id}/entries"]["get"]["parameters"].is_array());
    assert!(spec["components"]["securitySchemes"]["account_key"].is_object());
    assert!(spec["components"]["schemas"]["ProposalView"].is_object());

    let response = app.client.get(app.url("/docs")).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert!(response.headers()["content-type"]
        .to_str()
        .unwrap()
        .starts_with("text/html"));
}
