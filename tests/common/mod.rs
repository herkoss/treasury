#![allow(dead_code)]

use reqwest::{Client, Method, RequestBuilder, Response};
use serde_json::{json, Value};
use sqlx::PgPool;
use tokio::net::TcpListener;
use treasury::utils::key_hash;

pub struct TestApp {
    pub addr: String,
    pub client: Client,
    pub operator_key: String,
}

pub struct Party {
    pub id: String,
    pub key: String,
}

impl TestApp {
    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.addr)
    }

    pub fn request(&self, method: Method, path: &str) -> RequestBuilder {
        self.client.request(method, self.url(path))
    }

    pub fn as_account(&self, key: &str, method: Method, path: &str) -> RequestBuilder {
        self.request(method, path).header("x-account-key", key)
    }

    pub async fn create_asset(&self, code: &str, scale: u8) -> Response {
        self.as_account(&self.operator_key, Method::POST, "/assets")
            .json(&json!({ "code": code, "name": code, "scale": scale }))
            .send()
            .await
            .unwrap()
    }

    pub async fn get_assets(&self) -> Response {
        self.as_account(&self.operator_key, Method::GET, "/assets")
            .send()
            .await
            .unwrap()
    }

    pub async fn get_asset(&self, code: &str) -> Response {
        self.as_account(&self.operator_key, Method::GET, &format!("/assets/{code}"))
            .send()
            .await
            .unwrap()
    }

    pub async fn create_account(&self, name: &str) -> Party {
        let response = self
            .as_account(&self.operator_key, Method::POST, "/accounts")
            .json(&json!({ "name": name }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 201);
        let body: Value = response.json().await.unwrap();
        Party {
            id: body["id"].as_str().unwrap().to_string(),
            key: body["key"].as_str().unwrap().to_string(),
        }
    }

    pub async fn propose(&self, key: &str, body: Value) -> Response {
        self.as_account(key, Method::POST, "/transactions")
            .json(&body)
            .send()
            .await
            .unwrap()
    }

    pub async fn act(&self, key: &str, id: &str, action: &str) -> Response {
        self.as_account(key, Method::POST, &format!("/transactions/{id}/{action}"))
            .send()
            .await
            .unwrap()
    }

    pub async fn proposal(&self, key: &str, id: &str) -> Response {
        self.as_account(key, Method::GET, &format!("/transactions/{id}"))
            .send()
            .await
            .unwrap()
    }

    pub async fn account_view(&self, key: &str, account: &str, view: &str) -> Value {
        let response = self
            .as_account(key, Method::GET, &format!("/accounts/{account}/{view}"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        response.json().await.unwrap()
    }

    pub async fn balances(&self, key: &str, account: &str) -> Value {
        self.account_view(key, account, "balances").await
    }

    pub async fn entries(&self, key: &str, account: &str) -> Value {
        self.account_view(key, account, "entries").await
    }

    pub async fn inbox(&self, key: &str, account: &str) -> Value {
        self.account_view(key, account, "inbox").await
    }

    pub async fn outbox(&self, key: &str, account: &str) -> Value {
        self.account_view(key, account, "outbox").await
    }

    pub async fn deposit(&self, to: &str, asset: &str, amount: &str) -> Value {
        let response = self
            .propose(
                &self.operator_key,
                legs([leg("@issuer", to, asset, amount)]),
            )
            .await;
        assert_eq!(response.status(), 201);
        response.json().await.unwrap()
    }
}

pub fn leg(from: &str, to: &str, asset: &str, amount: &str) -> Value {
    json!({ "from": from, "to": to, "asset": asset, "amount": amount })
}

pub fn legs<const N: usize>(legs: [Value; N]) -> Value {
    json!({ "legs": Vec::from(legs) })
}

pub fn balance<'a>(balances: &'a Value, asset: &str) -> &'a Value {
    balances
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["asset"] == asset)
        .unwrap_or(&Value::Null)
}

pub fn approval<'a>(proposal: &'a Value, account: &str) -> &'a Value {
    proposal["required_approvals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["account"] == account)
        .unwrap_or(&Value::Null)
}

pub async fn spawn_app(pool: PgPool) -> TestApp {
    let state = treasury::http::AppState { pool: pool.clone() };

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = format!("http://{}", listener.local_addr().unwrap());
    let key = hex::encode(rand::random::<[u8; 32]>());
    let hash = key_hash(&key);
    treasury::db::ensure_issuer(&pool, &hash).await.unwrap();

    tokio::spawn(async move { treasury::http::serve(listener, state).await.unwrap() });

    TestApp {
        addr,
        client: Client::new(),
        operator_key: key,
    }
}

pub async fn alice_and_bob(app: &TestApp) -> (Party, Party) {
    assert_eq!(app.create_asset("USD", 2).await.status(), 201);
    assert_eq!(app.create_asset("XAU", 3).await.status(), 201);
    let alice = app.create_account("alice").await;
    let bob = app.create_account("bob").await;
    app.deposit(&alice.id, "USD", "1000").await;
    app.deposit(&bob.id, "XAU", "10").await;
    (alice, bob)
}
