use reqwest::Client;
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
