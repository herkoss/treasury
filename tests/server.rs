use reqwest::{Client, Method, RequestBuilder, Response};
use sqlx::PgPool;
use tokio::net::TcpListener;
use treasury::utils::key_hash;

struct TestApp {
    addr: String,
    client: Client,
    operator_key: String,
}

impl TestApp {
    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.addr)
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        self.client.request(method, self.url(path))
    }

    async fn create_asset(&self, code: &str, scale: u8) -> Response {
        self.request(Method::POST, "/assets")
            .header("x-account-key", &self.operator_key)
            .json(&serde_json::json!({ "code": code, "name": code, "scale": scale }))
            .send()
            .await
            .unwrap()
    }
}

async fn spawn_app(pool: PgPool) -> TestApp {
    let state = treasury::http::AppState { pool: pool.clone() };

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = format!("http://{}", listener.local_addr().unwrap());
    let key = hex::encode(rand::random::<[u8; 32]>());
    let hash = key_hash(&key);
    treasury::db::ensure_issuer(&pool, &hash).await.unwrap();

    tokio::spawn(async move { treasury::http::serve(listener, state).await.unwrap() });

    let client = Client::new();

    TestApp {
        addr,
        client,
        operator_key: key,
    }
}

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
